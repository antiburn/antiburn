use super::*;
use antiburn_local::platform::environment::DiscoveryEnvironment;
use std::collections::HashSet;
use std::io::Write;
use std::sync::Mutex;

/// A synthetic Claude store: `<home>/.claude/projects/<encoded>/<id>.jsonl`.
/// Every value is fictional; the shapes are what the engine's scanner reads.
fn write_claude_session(home: &std::path::Path, session_id: &str) -> std::path::PathBuf {
    let project = home
        .join(".claude")
        .join("projects")
        .join("-home-avery-code-widgets");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("{session_id}.jsonl"));
    std::fs::write(
        &path,
        format!(
            concat!(
                r#"{{"type":"summary","summary":"Wire the tray popover"}}"#,
                "\n",
                r#"{{"session_id":"{id}","cwd":"/home/avery/code/widgets","type":"user","#,
                r#""timestamp":"2026-08-01T10:00:00Z"}}"#,
                "\n",
                r#"{{"type":"assistant","timestamp":"2026-08-01T10:01:00Z","#,
                r#""message":{{"role":"assistant","model":"claude-opus-4-6","#,
                r#""usage":{{"input_tokens":120,"output_tokens":40}}}}}}"#,
                "\n",
            ),
            id = session_id
        ),
    )
    .unwrap();
    path
}

fn write_opencode_provider_db(home: &std::path::Path, session_id: &str) -> std::path::PathBuf {
    let path = home.join("opencode.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session (
                 id TEXT PRIMARY KEY, project_id TEXT NOT NULL, parent_id TEXT,
                 directory TEXT NOT NULL, title TEXT NOT NULL, version TEXT NOT NULL,
                 time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
             );
             CREATE TABLE message (
                 id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                 time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
             );
             CREATE TABLE part (
                 id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL,
                 time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
             );",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO session VALUES (?1, 'synthetic-project', NULL, '/repo',
                                          'Synthetic session', '1', 100, 120, '{}')",
            [session_id],
        )
        .unwrap();
    path
}

/// A synthetic OpenCode database with a parent session and a fork of it,
/// shaped so `db_fork_parent` (the engine's own database heuristic) finds
/// the relationship: the child's title carries the parent's title plus a
/// `(fork #N)` suffix, and the child's first two visible messages repeat the
/// parent's exactly before continuing with one message of its own.
fn write_opencode_fork_provider_db(
    home: &std::path::Path,
    parent_id: &str,
    child_id: &str,
) -> std::path::PathBuf {
    let path = home.join("opencode.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session (
                 id TEXT PRIMARY KEY, project_id TEXT NOT NULL, parent_id TEXT,
                 directory TEXT NOT NULL, title TEXT NOT NULL, version TEXT NOT NULL,
                 time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
             );
             CREATE TABLE message (
                 id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                 time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
             );
             CREATE TABLE part (
                 id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL,
                 time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
             );",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO session VALUES (?1, 'synthetic-project', NULL, '/repo',
                                          'Investigate the failing build', '1', 100, 120, '{}')",
            [parent_id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO session VALUES (?1, 'synthetic-project', NULL, '/repo',
                                          'Investigate the failing build (fork #1)', '1', 200, 220, '{}')",
            [child_id],
        )
        .unwrap();
    let insert_visible = |session_id: &str, suffix: &str, created: i64, role: &str, text: &str| {
        let message_id = format!("msg-{session_id}-{suffix}");
        let part_id = format!("part-{session_id}-{suffix}");
        connection
            .execute(
                "INSERT INTO message VALUES (?1, ?2, ?3, ?3, ?4)",
                rusqlite::params![
                    message_id,
                    session_id,
                    created,
                    format!(r#"{{"role":"{role}"}}"#)
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                rusqlite::params![
                    part_id,
                    message_id,
                    session_id,
                    created,
                    format!(r#"{{"type":"text","text":"{text}"}}"#)
                ],
            )
            .unwrap();
    };
    insert_visible(parent_id, "1", 100, "user", "Investigate the failing build");
    insert_visible(
        parent_id,
        "2",
        110,
        "assistant",
        "Looking into the logs now",
    );
    insert_visible(child_id, "1", 200, "user", "Investigate the failing build");
    insert_visible(child_id, "2", 210, "assistant", "Looking into the logs now");
    insert_visible(child_id, "3", 220, "user", "Also check the flaky test");
    path
}

/// A synthetic Codex rollout: `<home>/.codex/sessions/YYYY/MM/DD/...jsonl`.
fn write_codex_session(home: &std::path::Path, session_id: &str) -> std::path::PathBuf {
    let day = home
        .join(".codex")
        .join("sessions")
        .join("2026")
        .join("08")
        .join("01");
    std::fs::create_dir_all(&day).unwrap();
    let path = day.join(format!("rollout-2026-08-01T10-00-00-{session_id}.jsonl"));
    std::fs::write(
        &path,
        format!(
            concat!(
                r#"{{"timestamp":"2026-08-01T10:00:00Z","type":"session_meta","#,
                r#""payload":{{"id":"{id}","cwd":"/home/avery/code/gadgets"}}}}"#,
                "\n",
                r#"{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"Fallback transcript request"}}]}}}}"#,
                "\n",
            ),
            id = session_id
        ),
    )
    .unwrap();
    path
}

fn write_codex_fork_session(
    home: &std::path::Path,
    session_id: &str,
    parent_session_id: &str,
) -> std::path::PathBuf {
    let path = write_codex_session(home, session_id);
    let header = serde_json::json!({
        "timestamp": "2026-08-01T10:00:00Z",
        "type": "session_meta",
        "payload": {
            "id": session_id,
            "forked_from_id": parent_session_id,
            "cwd": "/home/avery/code/gadgets",
            "source": "cli",
            "thread_source": "user",
        }
    });
    std::fs::write(&path, format!("{header}\n")).unwrap();
    path
}

fn log(agent: AgentKind, path: std::path::PathBuf, updated_at: i64) -> SessionLog {
    SessionLog {
        agent_type: agent,
        source: SessionSource::File(path),
        updated_at: Some(updated_at),
        environment: DiscoveryEnvironment::Native,
    }
}

#[test]
fn only_native_direct_agents_are_eligible_for_indexed_title_lookups() {
    let direct = [AgentKind::Claude, AgentKind::Codex, AgentKind::OpenCode];
    for agent in AgentKind::ALL {
        let native = SessionLog {
            agent_type: *agent,
            source: SessionSource::Inline {
                label: "synthetic".into(),
                content: String::new(),
            },
            updated_at: None,
            environment: DiscoveryEnvironment::Native,
        };
        assert_eq!(
            should_lookup_indexed_title(&native),
            direct.contains(agent),
            "unexpected lookup route for {agent}"
        );

        let in_wsl = SessionLog {
            environment: DiscoveryEnvironment::Wsl {
                distribution: "SyntheticLinux".into(),
                user: "avery".into(),
            },
            ..native
        };
        assert!(
            !should_lookup_indexed_title(&in_wsl),
            "WSL {agent} must not query native stores"
        );
    }
}

#[test]
fn direct_titles_are_authoritative_and_keep_their_source() {
    let first = select_title_pair(
        Some(ResolvedTitle::new(
            "Generated session name",
            TitleSource::AiGenerated,
        )),
        Some("<injected transcript context>".into()),
        Some(TitleSource::FirstMessage),
        &AgentKind::Codex,
        None,
    );
    assert_eq!(
        first,
        (
            Some("Generated session name".into()),
            Some("aiGenerated".into())
        )
    );

    let renamed = select_title_pair(
        Some(ResolvedTitle::new(
            "Reader renamed session",
            TitleSource::UserRename,
        )),
        Some("old transcript fallback".into()),
        Some(TitleSource::FirstMessage),
        &AgentKind::Codex,
        None,
    );
    assert_eq!(
        renamed,
        (
            Some("Reader renamed session".into()),
            Some("userRename".into())
        )
    );

    let transcript_fallback = select_title_pair(
        Some(ResolvedTitle::new(
            "<recommended_plugins> injected context",
            TitleSource::FirstMessage,
        )),
        None,
        None,
        &AgentKind::Claude,
        Some(concat!(
            r#"{"type":"user","message":{"role":"user","content":"<recommended_plugins> injected context"}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"Reader's actual request"}}"#,
            "\n",
        )),
    );
    assert_eq!(
        transcript_fallback,
        (
            Some("Reader's actual request".into()),
            Some("firstMessage".into())
        )
    );
}

#[test]
fn a_direct_lookup_miss_keeps_the_transcript_fallback_pair() {
    assert_eq!(
        select_title_pair(
            None,
            Some("First reader request".into()),
            Some(TitleSource::FirstMessage),
            &AgentKind::Codex,
            None,
        ),
        (
            Some("First reader request".into()),
            Some("firstMessage".into())
        )
    );
}

#[tokio::test]
async fn a_native_codex_title_refreshes_while_wsl_keeps_its_own_fallback() {
    let home = tempfile::TempDir::new().unwrap();
    let session_id = "same-id-in-two-environments";
    let path = write_codex_session(home.path(), session_id);
    let native_log = log(AgentKind::Codex, path.clone(), 1_800_000_000);
    let store = crate::store::Store::open_in_memory(home.path()).unwrap();

    for (title, source) in [
        ("Indexed session name", TitleSource::AiGenerated),
        ("Reader renamed session", TitleSource::UserRename),
    ] {
        let DescribeOutcome::Session(record) = describe_one(
            native_log.clone(),
            home.path(),
            Some(ResolvedTitle::new(title, source)),
        )
        .await
        else {
            panic!("native Codex session should be described");
        };
        store
            .upsert_sessions(&[*record], &agents::evidence_cohort())
            .unwrap();
    }

    let native = store
        .session(&SessionKey::new("native", "codex", session_id))
        .unwrap()
        .expect("native session");
    assert_eq!(native.title.as_deref(), Some("Reader renamed session"));
    assert_eq!(native.title_source.as_deref(), Some("userRename"));

    let wsl_log = SessionLog {
        environment: DiscoveryEnvironment::Wsl {
            distribution: "SyntheticLinux".into(),
            user: "avery".into(),
        },
        ..log(AgentKind::Codex, path, 1_800_000_100)
    };
    let DescribeOutcome::Session(wsl) = describe_one(
        wsl_log,
        home.path(),
        // A same-id native hit is available but must be ignored for WSL.
        Some(ResolvedTitle::new(
            "Native title must not leak",
            TitleSource::UserRename,
        )),
    )
    .await
    else {
        panic!("WSL Codex session should be described");
    };
    assert_eq!(wsl.key.environment_key, "wsl:syntheticlinux");
    assert_eq!(wsl.title.as_deref(), Some("Fallback transcript request"));
    assert_eq!(wsl.title_source.as_deref(), Some("firstMessage"));
}

#[tokio::test]
async fn a_codex_fork_records_its_parent_during_the_scan() {
    let home = tempfile::TempDir::new().unwrap();
    let parent_session_id = "parent-session";
    let child_session_id = "child-session";
    let path = write_codex_fork_session(home.path(), child_session_id, parent_session_id);

    let DescribeOutcome::Session(child) = describe_one(
        log(AgentKind::Codex, path, 1_800_000_000),
        home.path(),
        None,
    )
    .await
    else {
        panic!("Codex fork should be described");
    };
    assert_eq!(
        child.fork_parent_session_id.as_deref(),
        Some(parent_session_id)
    );

    let store = crate::store::Store::open_in_memory(home.path()).unwrap();
    store
        .upsert_sessions(
            &[record("codex", parent_session_id, Some(1_799_999_000))],
            &agents::evidence_cohort(),
        )
        .unwrap();
    store
        .upsert_sessions(std::slice::from_ref(&child), &agents::evidence_cohort())
        .unwrap();

    assert_eq!(
        store
            .fork_children(&SessionKey::new("native", "codex", parent_session_id))
            .unwrap(),
        vec![child_session_id.to_string()]
    );
}

#[tokio::test]
async fn describing_transcripts_recovers_identity_title_and_working_directory() {
    let home = tempfile::TempDir::new().unwrap();
    let claude = write_claude_session(home.path(), "11111111-2222-3333-4444-555555555555");
    let codex = write_codex_session(home.path(), "codex-abc");

    let records = describe(
        vec![
            log(AgentKind::Claude, claude, 1_800_000_000),
            log(AgentKind::Codex, codex, 1_800_000_100),
        ],
        home.path(),
        &HashSet::new(),
    )
    .await;

    assert_eq!(records.records.len(), 2);
    assert!(records.rejected.is_empty());

    let claude = records
        .records
        .iter()
        .find(|record| record.key.agent == "claude-code")
        .expect("a claude record");
    assert_eq!(
        claude.key.session_id,
        "11111111-2222-3333-4444-555555555555"
    );
    assert_eq!(claude.key.environment_key, "native");
    assert_eq!(claude.cwd.as_deref(), Some("/home/avery/code/widgets"));
    assert_eq!(claude.title.as_deref(), Some("Wire the tray popover"));
    assert_eq!(claude.source_kind, "file");
    assert_eq!(claude.subagent_count, 0);
    assert!(
        claude
            .source_fingerprint
            .as_deref()
            .is_some_and(|value| value.starts_with("sv1:"))
    );

    let codex = records
        .records
        .iter()
        .find(|record| record.key.agent == "codex")
        .expect("a codex record");
    assert_eq!(codex.key.session_id, "codex-abc");
    assert_eq!(codex.cwd.as_deref(), Some("/home/avery/code/gadgets"));
    assert!(matches!(
        codex.surface.as_str(),
        "cli" | "ide_desktop" | "unknown"
    ));
}

#[tokio::test]
async fn describing_a_claude_session_reads_the_head_once() {
    let home = tempfile::TempDir::new().unwrap();
    let path = write_claude_session(home.path(), "one-head-read");
    antiburn_local::discovery::track_head_reads(&path);

    let outcome = describe_one(
        log(AgentKind::Claude, path.clone(), 1_800_000_000),
        home.path(),
        None,
    )
    .await;

    assert!(matches!(outcome, DescribeOutcome::Session(_)));
    assert_eq!(antiburn_local::discovery::take_tracked_head_reads(&path), 1);
}

#[tokio::test]
async fn a_second_pass_over_an_unchanged_source_performs_no_head_read() {
    let home = tempfile::TempDir::new().unwrap();
    let path = write_claude_session(home.path(), "unchanged-source");
    antiburn_local::discovery::track_head_reads(&path);
    let store = crate::store::Store::open_in_memory(home.path()).unwrap();

    let first = describe_with_states(
        vec![log(AgentKind::Claude, path.clone(), 1_800_000_000)],
        home.path(),
        &HashSet::new(),
        &store.session_records().unwrap(),
    )
    .await;
    assert_eq!(first.records.len(), 1);
    assert_eq!(antiburn_local::discovery::take_tracked_head_reads(&path), 1);
    // `take_tracked_head_reads` deregisters the path, so each phase
    // re-arms tracking for the read count it is about to check.
    antiburn_local::discovery::track_head_reads(&path);
    store
        .upsert_sessions(&first.records, &agents::evidence_cohort())
        .unwrap();

    // Second pass, source unchanged: the stored record is reused verbatim
    // and the transcript is never opened.
    let second = describe_with_states(
        vec![log(AgentKind::Claude, path.clone(), 1_800_000_100)],
        home.path(),
        &HashSet::new(),
        &store.session_records().unwrap(),
    )
    .await;
    assert_eq!(second.records.len(), 1);
    assert_eq!(second.records[0], first.records[0]);
    assert_eq!(antiburn_local::discovery::take_tracked_head_reads(&path), 0);
    antiburn_local::discovery::track_head_reads(&path);
    store
        .upsert_sessions(&second.records, &agents::evidence_cohort())
        .unwrap();

    // A genuine append changes the cursor and forces a real read.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"type\":\"assistant\",\"timestamp\":\"2026-08-01T10:05:00Z\"}\n")
        .unwrap();
    let third = describe_with_states(
        vec![log(AgentKind::Claude, path.clone(), 1_800_000_200)],
        home.path(),
        &HashSet::new(),
        &store.session_records().unwrap(),
    )
    .await;
    assert_eq!(third.records.len(), 1);
    assert_eq!(antiburn_local::discovery::take_tracked_head_reads(&path), 1);
}

#[tokio::test]
async fn an_mtime_row_is_described_again_when_only_its_mtime_moved() {
    let home = tempfile::TempDir::new().unwrap();
    let path = write_claude_session(home.path(), "mtime-source");
    let store = crate::store::Store::open_in_memory(home.path()).unwrap();

    let first = describe_with_states(
        vec![log(AgentKind::Claude, path.clone(), 1_800_000_000)],
        home.path(),
        &HashSet::new(),
        &store.session_records().unwrap(),
    )
    .await;
    assert_eq!(first.records.len(), 1);
    // Simulate a row whose activity fell back to the file mtime.
    let mut stored = first.records[0].clone();
    stored.activity_source = "mtime".into();
    stored.updated_at_epoch = Some(1_800_000_000);
    store
        .upsert_sessions(&[stored], &agents::evidence_cohort())
        .unwrap();

    // Same size and same mtime: reused without a read.
    antiburn_local::discovery::track_head_reads(&path);
    let same = describe_with_states(
        vec![log(AgentKind::Claude, path.clone(), 1_800_000_000)],
        home.path(),
        &HashSet::new(),
        &store.session_records().unwrap(),
    )
    .await;
    assert_eq!(antiburn_local::discovery::take_tracked_head_reads(&path), 0);
    assert_eq!(same.records[0].activity_source, "mtime");

    // Same size but a newer mtime: the pass must describe it again.
    antiburn_local::discovery::track_head_reads(&path);
    let moved = describe_with_states(
        vec![log(AgentKind::Claude, path.clone(), 1_800_000_100)],
        home.path(),
        &HashSet::new(),
        &store.session_records().unwrap(),
    )
    .await;
    assert_eq!(antiburn_local::discovery::take_tracked_head_reads(&path), 1);
    assert_eq!(moved.records[0].activity_source, "event");
}

#[test]
fn the_activity_cursor_format_is_pinned() {
    let parent = std::path::Path::new("/home/avery/.claude/projects/demo/session.jsonl");
    let child_a = std::path::PathBuf::from(
        "/home/avery/.claude/projects/demo/session/subagents/agent-a.jsonl",
    );
    let child_b = std::path::PathBuf::from(
        "/home/avery/.claude/projects/demo/session/subagents/agent-b.jsonl",
    );
    let cursor = activity_cursor(parent, 42, &[(child_a, Some(7)), (child_b, None)]);
    assert_eq!(
        cursor,
        concat!(
            r#"[["child","/home/avery/.claude/projects/demo/session/subagents/agent-a.jsonl","7"],"#,
            r#"["child","/home/avery/.claude/projects/demo/session/subagents/agent-b.jsonl","missing"],"#,
            r#"["parent","/home/avery/.claude/projects/demo/session.jsonl","42"]]"#
        )
    );
}

#[tokio::test]
async fn describing_an_opencode_provider_db_does_not_render_the_transcript() {
    let home = tempfile::TempDir::new().unwrap();
    let session_id = "opencode-provider-db";
    let db_path = write_opencode_provider_db(home.path(), session_id);
    let log = SessionLog {
        agent_type: AgentKind::OpenCode,
        source: SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path: db_path.clone(),
            session_id: session_id.to_string(),
        },
        updated_at: Some(120),
        environment: DiscoveryEnvironment::Native,
    };
    antiburn_local::discovery::track_provider_db_renders(&db_path);

    let outcome = describe_one(log, home.path(), None).await;

    assert!(matches!(outcome, DescribeOutcome::Session(_)));
    assert_eq!(
        antiburn_local::discovery::take_tracked_provider_db_renders(&db_path),
        0
    );
}

#[tokio::test]
async fn describing_an_opencode_fork_finds_its_parent_without_rendering_either_transcript() {
    let home = tempfile::TempDir::new().unwrap();
    let parent_id = "ses-parent";
    let child_id = "ses-child";
    let db_path = write_opencode_fork_provider_db(home.path(), parent_id, child_id);
    let log = SessionLog {
        agent_type: AgentKind::OpenCode,
        source: SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path: db_path.clone(),
            session_id: child_id.to_string(),
        },
        updated_at: Some(220),
        environment: DiscoveryEnvironment::Native,
    };
    antiburn_local::discovery::track_provider_db_renders(&db_path);

    let DescribeOutcome::Session(record) = describe_one(log, home.path(), None).await else {
        panic!("session should be described");
    };

    assert_eq!(record.fork_parent_session_id.as_deref(), Some(parent_id));
    // `db_fork_parent` finds the relationship from the database's own rows —
    // describe never has to render either session's transcript for it.
    assert_eq!(
        antiburn_local::discovery::take_tracked_provider_db_renders(&db_path),
        0
    );
}

#[tokio::test]
async fn a_consumed_provider_db_preview_is_rendered() {
    let home = tempfile::TempDir::new().unwrap();
    let session_id = "consumed-provider-db";
    let db_path = write_opencode_provider_db(home.path(), session_id);
    let log = SessionLog {
        agent_type: AgentKind::Claude,
        source: SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path: db_path.clone(),
            session_id: session_id.to_string(),
        },
        updated_at: Some(120),
        environment: DiscoveryEnvironment::Native,
    };
    antiburn_local::discovery::track_provider_db_renders(&db_path);

    let read = session_log_read(&log).await.expect("source read");

    assert!(read.content.is_some());
    assert_eq!(
        antiburn_local::discovery::take_tracked_provider_db_renders(&db_path),
        1
    );
}

#[tokio::test]
async fn an_inline_claude_subagent_is_rejected_on_the_scan_path() {
    let content = concat!(
        r#"{"type":"user","sessionId":"inline-subagent","isSidechain":true,"agentId":"agent-child","message":{"role":"user","content":"Investigate the failed deployment"}}"#,
        "\n",
    );
    let log = SessionLog {
        agent_type: AgentKind::Claude,
        source: SessionSource::Inline {
            label: "inline-subagent".to_string(),
            content: content.to_string(),
        },
        updated_at: Some(1_800_000_000),
        environment: DiscoveryEnvironment::Native,
    };

    assert!(matches!(
        describe_one(log, std::path::Path::new("/tmp"), None).await,
        DescribeOutcome::Subagent(key)
            if key == SessionKey::new("native", "claude-code", "inline-subagent")
    ));
}

#[tokio::test]
async fn a_descriptor_takes_the_metadata_session_id() {
    let home = tempfile::TempDir::new().unwrap();
    let path = home.path().join("recovered-file-name.jsonl");
    std::fs::write(
        &path,
        r#"{"type":"user","sessionId":"metadata-id","cwd":"/repo"}
"#,
    )
    .unwrap();

    let DescribeOutcome::Session(record) =
        describe_one(log(AgentKind::Claude, path, 100), home.path(), None).await
    else {
        panic!("session should be described");
    };

    assert_eq!(record.key.session_id, "metadata-id");
    assert!(record.source_fingerprint.is_some());
}

#[tokio::test]
async fn a_descriptor_falls_back_to_the_recovered_id() {
    let home = tempfile::TempDir::new().unwrap();
    let path = home.path().join("recovered-id.jsonl");
    std::fs::write(&path, "not json\n").unwrap();

    let DescribeOutcome::Session(record) =
        describe_one(log(AgentKind::Pi, path, 100), home.path(), None).await
    else {
        panic!("session should be described");
    };

    assert_eq!(record.key.session_id, "recovered-id");
    assert!(record.source_fingerprint.is_some());
}

#[tokio::test]
async fn an_empty_session_id_is_skipped() {
    let log = SessionLog {
        agent_type: AgentKind::Claude,
        source: SessionSource::Inline {
            label: String::new(),
            content: "{}".to_string(),
        },
        updated_at: None,
        environment: DiscoveryEnvironment::Native,
    };

    assert!(matches!(
        describe_one(log, std::path::Path::new("/tmp"), None).await,
        DescribeOutcome::Skip
    ));
}

#[tokio::test]
async fn an_appended_transcript_produces_a_different_fingerprint() {
    let home = tempfile::TempDir::new().unwrap();
    let path = write_claude_session(home.path(), "changing-session");
    let DescribeOutcome::Session(first) =
        describe_one(log(AgentKind::Claude, path.clone(), 100), home.path(), None).await
    else {
        panic!("session should be described");
    };
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"type\":\"assistant\"}\n")
        .unwrap();
    let DescribeOutcome::Session(second) =
        describe_one(log(AgentKind::Claude, path, 101), home.path(), None).await
    else {
        panic!("session should be described");
    };

    assert_ne!(first.source_fingerprint, second.source_fingerprint);
}

#[tokio::test]
async fn a_non_native_codex_title_survives_the_scan_path() {
    let home = tempfile::TempDir::new().unwrap();
    let session_id = "wsl-indexed-title";
    let path = write_codex_session(home.path(), session_id);
    std::fs::write(
        home.path().join(".codex/session_index.jsonl"),
        format!(
            r#"{{"id":"{session_id}","thread_name":"Indexed WSL title"}}
"#
        ),
    )
    .unwrap();
    let log = SessionLog {
        environment: DiscoveryEnvironment::Wsl {
            distribution: "SyntheticLinux".into(),
            user: "avery".into(),
        },
        ..log(AgentKind::Codex, path, 100)
    };

    let DescribeOutcome::Session(record) = describe_one(log, home.path(), None).await else {
        panic!("session should be described");
    };

    assert_eq!(record.title.as_deref(), Some("Indexed WSL title"));
    assert_eq!(record.title_source.as_deref(), Some("aiGenerated"));
}

#[tokio::test]
async fn a_wsl_cwd_is_mapped_to_a_windows_path_in_the_scan_path() {
    let home = tempfile::TempDir::new().unwrap();
    let path = write_codex_session(home.path(), "wsl-cwd");
    let log = SessionLog {
        environment: DiscoveryEnvironment::Wsl {
            distribution: "SyntheticLinux".into(),
            user: "avery".into(),
        },
        ..log(AgentKind::Codex, path, 100)
    };

    let DescribeOutcome::Session(record) = describe_one(log, home.path(), None).await else {
        panic!("session should be described");
    };

    assert_eq!(
        record.cwd.as_deref(),
        Some(r"\\wsl.localhost\SyntheticLinux\home\avery\code\gadgets")
    );
}

#[tokio::test]
async fn an_opted_out_working_directory_never_reaches_the_store() {
    let home = tempfile::TempDir::new().unwrap();
    let claude = write_claude_session(home.path(), "aaaa-bbbb");
    let codex = write_codex_session(home.path(), "codex-abc");
    let logs = vec![
        log(AgentKind::Claude, claude, 1_800_000_000),
        log(AgentKind::Codex, codex, 1_800_000_100),
    ];

    // The engine's opt-out gate covers the directory and everything under it.
    let ignored = HashSet::from(["/home/avery/code/widgets".to_string()]);
    let records = describe(logs, home.path(), &ignored).await;

    assert_eq!(records.records.len(), 1);
    assert_eq!(records.records[0].key.agent, "codex");
}

#[tokio::test]
async fn a_described_pass_round_trips_through_the_store_and_is_idempotent() {
    let home = tempfile::TempDir::new().unwrap();
    let store = crate::store::Store::open_in_memory(home.path()).unwrap();
    let claude = write_claude_session(home.path(), "aaaa-bbbb");
    let codex = write_codex_session(home.path(), "codex-abc");

    for _ in 0..2 {
        let records = describe(
            vec![
                log(AgentKind::Claude, claude.clone(), 1_800_000_000),
                log(AgentKind::Codex, codex.clone(), 1_800_000_100),
            ],
            home.path(),
            &HashSet::new(),
        )
        .await;
        store
            .upsert_sessions(&records.records, &agents::evidence_cohort())
            .unwrap();
        for (agent, seen, cursor) in per_agent_totals(&records.records) {
            store.record_agent_scan(&agent, cursor, seen).unwrap();
        }
    }

    // A second pass over the same machine updates rather than duplicates.
    let stored = store.recent_sessions(0, 100).unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(
        stored[0].key.session_id, "codex-abc",
        "newest activity first"
    );

    let state = store.scan_state().unwrap();
    assert_eq!(state.len(), 2);
    assert!(
        state
            .iter()
            .all(|(_, completed, seen)| { completed.is_some() && *seen == 1 })
    );
}

#[tokio::test]
async fn an_idle_touched_transcript_heals_mtime_recency_and_then_uses_size_gate() {
    let home = tempfile::TempDir::new().unwrap();
    let path = home
        .path()
        .join(".claude/projects/-home-avery-code-widgets/old.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        concat!(
            r#"{"type":"user","sessionId":"old","cwd":"/home/avery/code/widgets","timestamp":"2026-06-26T21:20:00Z"}"#,
            "\n",
            r#"{"type":"custom-title","customTitle":"Renamed","timestamp":"2026-08-19T17:07:32Z"}"#,
            "\n",
            r#"{"type":"permission-mode","mode":"default","timestamp":"2026-08-19T17:07:33Z"}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-06-26T21:30:15Z"}"#,
            "\n",
        ),
    )
    .unwrap();
    let store = crate::store::Store::open_in_memory(home.path()).unwrap();

    // Append a housekeeping record larger than the bounded tail. The
    // preview still contains the old activity and should heal this
    // migrated mtime row on its first semantic scan.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(
            format!(
                r#"{{"type":"permission-mode","mode":"default","timestamp":"2026-08-19T17:08:00Z","padding":"{}"}}
"#,
                "x".repeat(300_000)
            )
            .as_bytes(),
        )
        .unwrap();

    // Simulate a row written by the old mtime-based scanner. The semantic
    // pass must replace it with the old meaningful transcript activity.
    let mut stale = record("claude-code", "old", Some(1_787_155_652));
    stale.source_label = path.to_string_lossy().into_owned();
    stale.activity_cursor = "legacy".into();
    stale.activity_source = "mtime".into();
    store
        .upsert_sessions(&[stale], &agents::evidence_cohort())
        .unwrap();

    let states = store.session_records().unwrap();
    let described = describe_with_states(
        vec![log(AgentKind::Claude, path.clone(), 1_787_155_652)],
        home.path(),
        &HashSet::new(),
        &states,
    )
    .await;
    let expected = time::OffsetDateTime::parse(
        "2026-06-26T21:30:15Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
    .unix_timestamp();
    assert_eq!(described.records[0].updated_at_epoch, Some(expected));
    assert_eq!(described.records[0].activity_source, "event");
    store
        .upsert_sessions(&described.records, &agents::evidence_cohort())
        .unwrap();
    let stored = store
        .session_records()
        .unwrap()
        .remove(&SessionActivityKey::new(
            "native",
            AgentKind::Claude.slug(),
            path.to_string_lossy().into_owned(),
        ))
        .expect("healed activity cursor");
    assert_eq!(stored.updated_at_epoch, Some(expected));
    assert_eq!(stored.activity_source, "event");

    // A harness appends housekeeping only. The changed size invalidates
    // the cursor, but the previous event seed survives the suffix parse
    // and prevents the new mtime from promoting the session.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(
            format!(
                r#"{{"type":"permission-mode","mode":"default","timestamp":"2026-08-19T17:08:00Z","padding":"{}"}}
"#,
                "x".repeat(300_000)
            )
            .as_bytes(),
        )
        .unwrap();
    let states = store.session_records().unwrap();
    let touched = describe_with_states(
        vec![log(AgentKind::Claude, path.clone(), 1_800_000_000)],
        home.path(),
        &HashSet::new(),
        &states,
    )
    .await;
    assert_eq!(touched.records[0].updated_at_epoch, Some(expected));
    assert_eq!(touched.records[0].activity_source, "event");

    // A later mtime-only touch now hits the unchanged-size cursor gate.
    let states = {
        store
            .upsert_sessions(&touched.records, &agents::evidence_cohort())
            .unwrap();
        store.session_records().unwrap()
    };
    let gated = describe_with_states(
        vec![log(AgentKind::Claude, path, 1_800_000_001)],
        home.path(),
        &HashSet::new(),
        &states,
    )
    .await;
    assert_eq!(gated.records[0].updated_at_epoch, Some(expected));
}

#[tokio::test]
async fn an_orchestrator_cursor_gates_unchanged_children_and_advances_on_child_growth() {
    let home = tempfile::TempDir::new().unwrap();
    let parent = write_claude_session(home.path(), "orchestrator");
    let child_dir = parent
        .parent()
        .unwrap()
        .join("orchestrator")
        .join("subagents");
    std::fs::create_dir_all(&child_dir).unwrap();
    let child = child_dir.join("agent-child.jsonl");
    std::fs::write(
        &child,
        r#"{"type":"assistant","timestamp":"2026-08-01T10:02:00Z"}
"#,
    )
    .unwrap();
    let store = crate::store::Store::open_in_memory(home.path()).unwrap();

    let first = describe_with_states(
        vec![log(AgentKind::Claude, parent.clone(), 1_800_000_000)],
        home.path(),
        &HashSet::new(),
        &store.session_records().unwrap(),
    )
    .await;
    assert_eq!(first.records[0].subagent_count, 1);
    let first_epoch = first.records[0].updated_at_epoch.unwrap();
    store
        .upsert_sessions(&first.records, &agents::evidence_cohort())
        .unwrap();

    // An mtime-only parent touch with an unchanged parent+child cursor is
    // served from the cached semantic event without reading either tail.
    let gated = describe_with_states(
        vec![log(AgentKind::Claude, parent.clone(), 1_900_000_000)],
        home.path(),
        &HashSet::new(),
        &store.session_records().unwrap(),
    )
    .await;
    assert_eq!(gated.records[0].updated_at_epoch, Some(first_epoch));

    // Appending genuine child work changes the aggregate cursor and
    // promotes the parent to the child's semantic event time.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&child)
        .unwrap()
        .write_all(
            br#"{"type":"assistant","timestamp":"2026-08-01T10:03:00Z"}
"#,
        )
        .unwrap();
    let advanced = describe_with_states(
        vec![log(AgentKind::Claude, parent, 1_900_000_001)],
        home.path(),
        &HashSet::new(),
        &store.session_records().unwrap(),
    )
    .await;
    let expected = time::OffsetDateTime::parse(
        "2026-08-01T10:03:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
    .unix_timestamp();
    assert_eq!(advanced.records[0].updated_at_epoch, Some(expected));
    assert!(advanced.records[0].updated_at_epoch.unwrap() > first_epoch);
}

/// A synthetic Claude sidechain transcript: `agentId` on every record and
/// `isSidechain: true`, written beside top-level sessions the way current
/// agent versions do.
fn write_claude_sidechain(home: &std::path::Path, agent_id: &str) -> std::path::PathBuf {
    let project = home
        .join(".claude")
        .join("projects")
        .join("-home-avery-code-widgets");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("agent-{agent_id}.jsonl"));
    std::fs::write(
        &path,
        format!(
            concat!(
                r#"{{"type":"user","isSidechain":true,"agentId":"{id}","#,
                r#""sessionId":"{id}","cwd":"/home/avery/code/widgets","#,
                r#""timestamp":"2026-08-01T10:00:00Z","#,
                r#""message":{{"role":"user","content":"subtask"}}}}"#,
                "\n",
            ),
            id = agent_id
        ),
    )
    .unwrap();
    path
}

#[tokio::test]
async fn a_sidechain_transcript_is_rejected_not_listed() {
    let home = tempfile::TempDir::new().unwrap();
    let parent = write_claude_session(home.path(), "11111111-2222-3333-4444-555555555555");
    let sidechain = write_claude_sidechain(home.path(), "aaaa-1111");

    let described = describe(
        vec![
            log(AgentKind::Claude, parent, 1_800_000_000),
            log(AgentKind::Claude, sidechain, 1_800_000_050),
        ],
        home.path(),
        &HashSet::new(),
    )
    .await;

    assert_eq!(described.records.len(), 1, "only the parent is listable");
    assert_eq!(described.rejected.len(), 1);
    assert_eq!(described.rejected[0].session_id, "aaaa-1111");
}

#[tokio::test]
async fn a_codex_subagent_thread_is_rejected_not_listed() {
    let home = tempfile::TempDir::new().unwrap();
    let day = home.path().join(".codex/sessions/2026/08/01");
    std::fs::create_dir_all(&day).unwrap();
    let path = day.join("rollout-2026-08-01T10-00-00-child-1.jsonl");
    std::fs::write(
        &path,
        concat!(
            r#"{"timestamp":"2026-08-01T10:00:00Z","type":"session_meta","#,
            r#""payload":{"id":"child-1","cwd":"/home/avery/code/gadgets","#,
            r#""parent_thread_id":"parent-9","thread_source":"subagent"}}"#,
            "\n",
        ),
    )
    .unwrap();

    let described = describe(
        vec![log(AgentKind::Codex, path, 1_800_000_000)],
        home.path(),
        &HashSet::new(),
    )
    .await;

    assert!(described.records.is_empty());
    assert_eq!(described.rejected.len(), 1);
    assert_eq!(described.rejected[0].session_id, "child-1");
}

#[tokio::test]
async fn a_rejected_transcript_evicts_its_stale_row_from_the_store() {
    let home = tempfile::TempDir::new().unwrap();
    let store = crate::store::Store::open_in_memory(home.path()).unwrap();
    // An earlier, ungated version of the app indexed the sidechain.
    store
        .upsert_sessions(
            &[record("claude-code", "aaaa-1111", Some(1_800_000_000))],
            &agents::evidence_cohort(),
        )
        .unwrap();
    assert_eq!(store.recent_sessions(0, 10).unwrap().len(), 1);

    let sidechain = write_claude_sidechain(home.path(), "aaaa-1111");
    let described = describe(
        vec![log(AgentKind::Claude, sidechain, 1_800_000_050)],
        home.path(),
        &HashSet::new(),
    )
    .await;
    for key in &described.rejected {
        store.delete_session(key).unwrap();
    }

    assert!(store.recent_sessions(0, 10).unwrap().is_empty());
}

/// A transcript whose first user message is an injected harness block:
/// the title must come from the first thing the reader actually typed.
#[tokio::test]
async fn an_injected_context_block_never_becomes_the_title() {
    let home = tempfile::TempDir::new().unwrap();
    let project = home
        .path()
        .join(".claude")
        .join("projects")
        .join("-home-avery-code-widgets");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.join("cccc-dddd.jsonl");
    std::fs::write(
        &path,
        concat!(
            r#"{"sessionId":"cccc-dddd","cwd":"/home/avery/code/widgets","type":"user","#,
            r#""timestamp":"2026-08-01T10:00:00Z","message":{"role":"user","#,
            r#""content":"<recommended_plugins> Here is a list of plugins that are recommended."}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-08-01T10:00:10Z","#,
            r#""message":{"role":"user","content":"Fix the tray popover anchoring"}}"#,
            "\n",
        ),
    )
    .unwrap();

    let described = describe(
        vec![log(AgentKind::Claude, path, 1_800_000_000)],
        home.path(),
        &HashSet::new(),
    )
    .await;

    assert_eq!(described.records.len(), 1);
    assert_eq!(
        described.records[0].title.as_deref(),
        Some("Fix the tray popover anchoring")
    );
}

/// A transcript that is nothing but injected context gets no title at all
/// — the row falls back to its path label rather than showing harness
/// text as if the reader wrote it.
#[tokio::test]
async fn a_transcript_with_only_injected_context_gets_no_title() {
    assert_eq!(
        sanitized_title(
            Some("<recommended_plugins> Here is a list".to_string()),
            &AgentKind::Claude,
            Some(concat!(
                r#"{"type":"user","message":{"role":"user","content":"<system-reminder>x</system-reminder>"}}"#,
                "\n",
            )),
        ),
        None
    );
    // Non-injected titles pass through untouched.
    assert_eq!(
        sanitized_title(Some("Fix the bug".to_string()), &AgentKind::Claude, None),
        Some("Fix the bug".to_string())
    );
    // "Caveat:" is the harness's resumed-session preamble, not the reader.
    assert_eq!(
        sanitized_title(
            Some("Caveat: the messages below were generated".to_string()),
            &AgentKind::Claude,
            None
        ),
        None
    );

    let home = tempfile::TempDir::new().unwrap();
    let project = home
        .path()
        .join(".claude/projects/-home-avery-code-widgets");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.join("only-context.jsonl");
    std::fs::write(
        &path,
        concat!(
            r#"{"sessionId":"only-context","cwd":"/home/avery/code/widgets","type":"user","message":{"role":"user","content":"<recommended_plugins> list"}}"#,
            "\n",
        ),
    )
    .unwrap();
    let described = describe(
        vec![log(AgentKind::Claude, path, 1_800_000_000)],
        home.path(),
        &HashSet::new(),
    )
    .await;
    assert_eq!(described.records.len(), 1);
    assert_eq!(described.records[0].title, None);
    assert_eq!(described.records[0].title_source, None);
}

#[tokio::test]
async fn a_transcript_with_no_embedded_id_falls_back_to_its_filename() {
    let home = tempfile::TempDir::new().unwrap();
    let path = home.path().join("orphan-session.jsonl");
    std::fs::write(&path, "not json at all\n").unwrap();

    let records = describe(
        vec![log(AgentKind::Pi, path, 1_800_000_000)],
        home.path(),
        &HashSet::new(),
    )
    .await;
    assert_eq!(records.records.len(), 1);
    assert_eq!(records.records[0].key.session_id, "orphan-session");
}

fn record(agent: &str, session_id: &str, updated_at: Option<i64>) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new("native", agent, session_id),
        source_kind: "file".into(),
        source_label: format!("/tmp/{session_id}.jsonl"),
        wsl_distro: None,
        title: None,
        title_source: None,
        cwd: None,
        surface: "cli".into(),
        updated_at_epoch: updated_at,
        activity_cursor: String::new(),
        activity_source: "mtime".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: None,
    }
}

#[test]
fn per_agent_totals_count_sessions_and_keep_the_newest_activity() {
    let records = vec![
        record("claude-code", "a", Some(1_000)),
        record("claude-code", "b", Some(3_000)),
        record("codex", "c", Some(2_000)),
        record("codex", "d", None),
    ];
    let totals = per_agent_totals(&records);
    assert_eq!(
        totals,
        vec![
            ("claude-code".to_string(), 2, Some(3_000)),
            ("codex".to_string(), 2, Some(2_000)),
        ]
    );
}

#[test]
fn a_pass_with_nothing_discovered_reports_no_agents() {
    assert!(per_agent_totals(&[]).is_empty());
}

#[test]
fn source_kinds_are_stable_wire_strings() {
    assert_eq!(
        source_kind(&SessionSource::File("/tmp/x.jsonl".into())),
        "file"
    );
    assert_eq!(
        source_kind(&SessionSource::Inline {
            label: "opencode:x".into(),
            content: String::new(),
        }),
        "inline"
    );
    assert_eq!(
        source_kind(&SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path: "/tmp/opencode.db".into(),
            session_id: "x".into(),
        }),
        "providerDb"
    );
}

#[test]
fn a_provider_database_session_recovers_its_id_from_the_source() {
    let log = SessionLog {
        agent_type: AgentKind::OpenCode,
        source: SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path: "/tmp/opencode.db".into(),
            session_id: "ses_123".into(),
        },
        updated_at: Some(1_000),
        environment: Default::default(),
    };
    assert_eq!(recovered_id(&log).as_deref(), Some("ses_123"));
}

#[test]
fn a_file_session_falls_back_to_its_filename_stem() {
    let log = SessionLog {
        agent_type: AgentKind::Claude,
        source: SessionSource::File("/home/avery/.claude/projects/demo/abc-123.jsonl".into()),
        updated_at: Some(1_000),
        environment: Default::default(),
    };
    assert_eq!(recovered_id(&log).as_deref(), Some("abc-123"));
}

#[test]
fn a_codex_rollout_recovers_its_canonical_uuid_before_title_prefetch() {
    let session_id = "01a01251-9875-7121-ac24-0d99fd8ccbe1";
    let log = SessionLog {
        agent_type: AgentKind::Codex,
        source: SessionSource::File(
            format!(
                "/home/avery/.codex/sessions/2026/08/18/rollout-2026-08-18T10-42-12-{session_id}.jsonl"
            )
            .into(),
        ),
        updated_at: Some(1_000),
        environment: Default::default(),
    };

    assert_eq!(recovered_id(&log).as_deref(), Some(session_id));
}

#[test]
fn an_on_demand_pass_starts_without_the_scheduler_gate() {
    let controller = ScanController::default();
    assert!(on_demand_start(&controller));
    assert!(!on_demand_start(&controller));
    controller.running.store(false, Ordering::SeqCst);
    assert!(on_demand_start(&controller));
}

#[test]
fn a_fresh_controller_reports_a_clean_initial_status() {
    let controller = ScanController::default();

    let status = controller.status();
    assert!(!status.running);
    assert_eq!(status.sessions, 0);
    assert!(status.error.is_none());
    assert!(!status.cancelled);
}

#[test]
fn a_cancel_request_only_applies_while_a_pass_is_running() {
    let controller = ScanController::default();

    // Nothing is running: a cancel would otherwise be remembered and would
    // kill the *next* pass, which is not what the reader asked for.
    controller.request_cancel();
    assert!(!controller.cancelled());

    controller.running.store(true, Ordering::SeqCst);
    controller.request_cancel();
    assert!(controller.cancelled());
}

#[test]
fn the_scheduler_ticks_at_the_fallback_rate_when_the_watcher_is_not_healthy() {
    assert_eq!(
        tick_for(&watch::WatcherStatus::default()),
        watch::FALLBACK_TICK
    );
    assert_eq!(
        tick_for(&watch::WatcherStatus {
            active: true,
            failed_roots: vec![std::path::PathBuf::from("/home/avery/.codex/sessions")],
        }),
        watch::FALLBACK_TICK
    );
    assert_eq!(
        tick_for(&watch::WatcherStatus {
            active: true,
            failed_roots: Vec::new(),
        }),
        TICK
    );
}

#[tokio::test]
async fn an_unchanged_pass_emits_nothing_and_reports_no_list_change() {
    let home = tempfile::TempDir::new().unwrap();
    let path = write_claude_session(home.path(), "steady");
    let store = crate::store::Store::open_in_memory(home.path()).unwrap();

    let first = describe_with_states(
        vec![log(AgentKind::Claude, path.clone(), 1_800_000_000)],
        home.path(),
        &HashSet::new(),
        &store.session_records().unwrap(),
    )
    .await;
    store
        .upsert_sessions(&first.records, &agents::evidence_cohort())
        .unwrap();

    // Same size, same mtime bucket: the source is reused, so nothing changed.
    let previous = store.session_records().unwrap();
    let second = describe_with_states(
        vec![log(AgentKind::Claude, path, 1_800_000_000)],
        home.path(),
        &HashSet::new(),
        &previous,
    )
    .await;
    assert!(second.changed.is_empty());
    assert!(!second.list_changed);

    let announced = Mutex::new(Vec::new());
    announce_changed_rows(
        &store,
        &second.changed,
        &previous,
        1_800_000_100,
        &|entry| {
            announced.lock().unwrap().push(entry);
        },
    );
    assert!(announced.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_moved_cursor_emits_exactly_one_entry_and_reports_no_list_change() {
    let home = tempfile::TempDir::new().unwrap();
    let path = write_claude_session(home.path(), "moving");
    let store = crate::store::Store::open_in_memory(home.path()).unwrap();

    let first = describe_with_states(
        vec![log(AgentKind::Claude, path.clone(), 1_800_000_000)],
        home.path(),
        &HashSet::new(),
        &store.session_records().unwrap(),
    )
    .await;
    store
        .upsert_sessions(&first.records, &agents::evidence_cohort())
        .unwrap();
    let previous = store.session_records().unwrap();

    // A genuine append moves the activity cursor: the row is known, but its
    // source changed, so it is re-described rather than reused.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"type\":\"assistant\",\"timestamp\":\"2026-08-01T10:05:00Z\"}\n")
        .unwrap();
    let second = describe_with_states(
        vec![log(AgentKind::Claude, path, 1_800_000_100)],
        home.path(),
        &HashSet::new(),
        &previous,
    )
    .await;
    assert_eq!(second.changed.len(), 1);
    assert!(!second.list_changed);
    store
        .upsert_sessions(&second.records, &agents::evidence_cohort())
        .unwrap();

    let announced = Mutex::new(Vec::new());
    announce_changed_rows(
        &store,
        &second.changed,
        &previous,
        1_800_000_200,
        &|entry| {
            announced.lock().unwrap().push(entry);
        },
    );
    let entries = announced.lock().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].session_id, "moving");
}

#[tokio::test]
async fn a_new_session_emits_nothing_and_reports_a_list_change() {
    let home = tempfile::TempDir::new().unwrap();
    let path = write_claude_session(home.path(), "fresh");
    let store = crate::store::Store::open_in_memory(home.path()).unwrap();
    let previous = store.session_records().unwrap();

    let described = describe_with_states(
        vec![log(AgentKind::Claude, path, 1_800_000_000)],
        home.path(),
        &HashSet::new(),
        &previous,
    )
    .await;
    assert_eq!(described.changed.len(), 1);
    assert!(described.list_changed);

    // A brand-new session has no row on screen to patch; the list's own
    // `list_changed` refetch is what picks it up, not this event.
    let announced = Mutex::new(Vec::new());
    announce_changed_rows(
        &store,
        &described.changed,
        &previous,
        1_800_000_100,
        &|entry| {
            announced.lock().unwrap().push(entry);
        },
    );
    assert!(announced.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_rejected_transcript_reports_a_list_change() {
    let home = tempfile::TempDir::new().unwrap();
    let sidechain = write_claude_sidechain(home.path(), "aaaa-1111");

    let described = describe(
        vec![log(AgentKind::Claude, sidechain, 1_800_000_000)],
        home.path(),
        &HashSet::new(),
    )
    .await;

    assert_eq!(described.rejected.len(), 1);
    assert!(described.list_changed);
}
