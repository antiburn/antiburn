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
async fn an_unchanged_rollout_accepts_a_fresh_indexed_title_without_a_head_read() {
    let home = tempfile::TempDir::new().unwrap();
    let path = write_codex_session(home.path(), "indexed-after-discovery");
    let log = log(AgentKind::Codex, path.clone(), 1_800_000_000);
    let DescribeOutcome::Session(cached) = describe_one(log.clone(), home.path(), None).await
    else {
        panic!("session should be described");
    };
    antiburn_local::discovery::track_head_reads(&path);

    let refreshed = reuse_unchanged_record(
        &log,
        Some(&cached),
        Some(&ResolvedTitle::new(
            "Review scan freshness",
            TitleSource::AiGenerated,
        )),
    )
    .await
    .expect("unchanged source should be reused");

    assert_eq!(refreshed.title.as_deref(), Some("Review scan freshness"));
    assert_eq!(refreshed.title_source.as_deref(), Some("aiGenerated"));
    assert_eq!(antiburn_local::discovery::take_tracked_head_reads(&path), 0);
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

#[tokio::test]
async fn claude_sidecar_changes_bypass_the_unchanged_source_gate() {
    let home = tempfile::TempDir::new().unwrap();
    let parent = write_claude_session(home.path(), "sidecar-source");
    let child_dir = parent.parent().unwrap().join("sidecar-source/subagents");
    std::fs::create_dir_all(&child_dir).unwrap();
    let child = child_dir.join("agent-child.jsonl");
    std::fs::write(&child, "{}\n").unwrap();
    let sidecar = child.with_extension("meta.json");
    let store = crate::store::Store::open_in_memory(home.path()).unwrap();
    let log = log(AgentKind::Claude, parent.clone(), 1_800_000_000);
    let first = describe_with_states(
        vec![log.clone()],
        home.path(),
        &HashSet::new(),
        &store.session_records().unwrap(),
    )
    .await;
    let mut previous = first.records[0].clone();
    for content in [
        Some(r#"{"toolUseId":"call-a"}"#),
        Some(r#"{"toolUseId":"call-b"}"#),
        None,
    ] {
        store
            .upsert_sessions(std::slice::from_ref(&previous), &agents::evidence_cohort())
            .unwrap();
        assert!(
            reuse_unchanged_record(&log, Some(&previous), None)
                .await
                .is_some()
        );
        match content {
            Some(content) => std::fs::write(&sidecar, content).unwrap(),
            None => std::fs::remove_file(&sidecar).unwrap(),
        }
        assert!(
            reuse_unchanged_record(&log, Some(&previous), None)
                .await
                .is_none()
        );
        let next = describe_with_states(
            vec![log.clone()],
            home.path(),
            &HashSet::new(),
            &store.session_records().unwrap(),
        )
        .await;
        assert_ne!(previous.activity_cursor, next.records[0].activity_cursor);
        assert_eq!(
            previous.source_fingerprint,
            next.records[0].source_fingerprint
        );
        assert_eq!(previous.updated_at_epoch, next.records[0].updated_at_epoch);
        previous = next.records[0].clone();
    }
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
fn a_fresh_indexed_title_updates_a_cached_record() {
    let mut cached = record("codex", "cached-title", Some(1_000));
    cached.title = Some("The first message".into());
    cached.title_source = Some("firstMessage".into());

    let changed = apply_indexed_title(
        &mut cached,
        &AgentKind::Codex,
        ResolvedTitle::new("Review scan freshness", TitleSource::AiGenerated),
    );

    assert!(changed);
    assert_eq!(cached.title.as_deref(), Some("Review scan freshness"));
    assert_eq!(cached.title_source.as_deref(), Some("aiGenerated"));
}

#[test]
fn an_equal_indexed_title_keeps_a_cached_record_unchanged() {
    let mut cached = record("codex", "steady-title", Some(1_000));
    cached.title = Some("Review scan freshness".into());
    cached.title_source = Some("aiGenerated".into());

    let changed = apply_indexed_title(
        &mut cached,
        &AgentKind::Codex,
        ResolvedTitle::new("Review scan freshness", TitleSource::AiGenerated),
    );

    assert!(!changed);
}

#[test]
fn records_to_persist_keeps_only_changed_or_returned_rows() {
    let a = record("claude-code", "a", Some(1_000));
    let b = record("claude-code", "b", Some(2_000));
    let c = record("codex", "c", Some(3_000));
    let records = vec![a.clone(), b.clone(), c.clone()];

    assert_eq!(records_to_persist(&records, &[], &[]), Vec::new());

    assert_eq!(
        records_to_persist(&records, std::slice::from_ref(&a.key), &[]),
        vec![a.clone()],
        "changed only"
    );

    assert_eq!(
        records_to_persist(&records, &[], std::slice::from_ref(&c.key)),
        vec![c.clone()],
        "returned only"
    );

    assert_eq!(
        records_to_persist(
            &records,
            &[a.key.clone(), b.key.clone()],
            std::slice::from_ref(&b.key),
        ),
        vec![a, b],
        "a key named by both changed and returned still yields one copy"
    );
}

#[test]
fn scoped_persistence_skips_unchanged_rows_without_calling_the_store() {
    let records = vec![record("claude-code", "steady", Some(1_000))];
    let calls = std::cell::Cell::new(0);

    let persisted = persist_changed_records(&records, &[], &[], |_| {
        calls.set(calls.get() + 1);
        Ok(())
    })
    .unwrap();

    assert!(persisted.is_none());
    assert_eq!(calls.get(), 0);
}

#[test]
fn scoped_persistence_writes_changed_new_and_returned_rows_once() {
    let changed = record("claude-code", "changed", Some(1_000));
    let new = record("claude-code", "new", Some(2_000));
    let returned = record("codex", "returned", Some(3_000));
    let unchanged = record("codex", "steady", Some(4_000));
    let records = vec![changed.clone(), new.clone(), returned.clone(), unchanged];
    let writes = Mutex::new(Vec::new());

    let persisted = persist_changed_records(
        &records,
        &[changed.key.clone(), new.key.clone()],
        std::slice::from_ref(&returned.key),
        |batch| {
            writes.lock().unwrap().push(batch.to_vec());
            Ok(batch.len())
        },
    )
    .unwrap();

    assert_eq!(persisted, Some(3), "the write's own result comes back");
    assert_eq!(
        writes.into_inner().unwrap(),
        vec![vec![changed, new, returned]]
    );
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

/// A scoped pass counts only its named agents, not the whole install, so its
/// count is not comparable to a full pass's count. Only a full pass may
/// report to analytics, on success or on failure.
#[test]
fn only_a_full_pass_reports_a_scan_outcome() {
    let scoped = PassScope::Agents(std::collections::BTreeSet::from([AgentKind::Claude]));

    assert_eq!(
        scan_report(&PassScope::Full, Some(1_000)),
        Some(Some(1_000))
    );
    assert_eq!(scan_report(&PassScope::Full, None), Some(None));
    assert_eq!(scan_report(&scoped, Some(3)), None, "a scoped success");
    assert_eq!(scan_report(&scoped, None), None, "a scoped failure");
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

#[tokio::test(start_paused = true)]
async fn a_second_request_before_the_scheduler_wakes_is_coalesced() {
    let controller = ScanController::default();
    controller.request(ScanTrigger::SettingsTransition);
    controller.request(ScanTrigger::ManualRescan);

    {
        let pending = controller
            .pending_trigger
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            matches!(pending.as_ref(), Some(ScanTrigger::SettingsTransition)),
            "the first trigger is kept, the second is dropped"
        );
    }

    // The two requests notify only once: this consumes that single permit...
    controller.kick.notified().await;
    // ...and a second wait finds nothing further queued.
    let second = tokio::time::timeout(Duration::from_millis(0), controller.kick.notified()).await;
    assert!(second.is_err(), "only one notify should have been queued");
}

#[test]
fn a_slow_scheduler_retains_one_bounded_merged_watcher_burst() {
    let controller = ScanController::default();
    for index in 0..10_000 {
        controller.push_burst(watch::WatchBurst {
            paths: vec![std::path::PathBuf::from(format!(
                "/tmp/{index:05}-{}",
                "x".repeat(1024)
            ))],
            events: 1,
            overflowed: false,
        });
    }

    let pending = controller
        .pending_burst
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let burst = pending.as_ref().expect("one merged burst stays pending");
    assert_eq!(burst.events, 10_000);
    assert!(burst.overflowed);
    assert!(burst.paths.len() <= watch::MAX_BURST_PATHS);
    assert!(
        burst
            .paths
            .iter()
            .map(|path| path.as_os_str().as_encoded_bytes().len())
            .sum::<usize>()
            <= watch::MAX_BURST_PATH_BYTES
    );
}

/// R4: the tick and the watcher triggers cannot plausibly have introduced a
/// repository the list has not already seen, so they alone skip the refresh.
#[test]
fn refreshes_repositories_is_false_only_for_the_tick_and_the_watcher_triggers() {
    let cheap = [
        ScanTrigger::Tick,
        ScanTrigger::WatcherAgents {
            agents: vec!["claude-code"],
        },
        ScanTrigger::WatcherOverflow,
    ];
    for trigger in &cheap {
        assert!(
            !trigger.refreshes_repositories(),
            "{} should not refresh repositories",
            trigger.label()
        );
    }

    let refreshing = [
        ScanTrigger::Launch,
        ScanTrigger::SettingsTransition,
        ScanTrigger::RepositoryToggle,
        ScanTrigger::ScanRootAdded,
        ScanTrigger::FolderAccessGranted,
        ScanTrigger::IndexCleared,
        ScanTrigger::ManualRescan,
    ];
    for trigger in &refreshing {
        assert!(
            trigger.refreshes_repositories(),
            "{} should refresh repositories",
            trigger.label()
        );
    }
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
    assert_eq!(tick_for_health(false), watch::FALLBACK_TICK);
    assert_eq!(tick_for_health(true), TICK);
}

#[tokio::test(start_paused = true)]
async fn watcher_health_transitions_update_the_deadline_without_postponing_it() {
    let last_full_pass = tokio::time::Instant::now();
    let healthy_deadline = last_full_pass + TICK;

    let degraded = deadline_after_health_change(healthy_deadline, last_full_pass, true, false);
    assert_eq!(degraded, last_full_pass + watch::FALLBACK_TICK);
    assert_eq!(
        deadline_after_health_change(degraded, last_full_pass, false, false),
        degraded,
        "identical degraded updates cannot postpone reconciliation"
    );
    assert_eq!(
        deadline_after_health_change(degraded, last_full_pass, false, true),
        healthy_deadline,
        "recovery restores the cadence from the last full pass"
    );
}

#[tokio::test(start_paused = true)]
async fn alternating_health_cannot_move_reconciliation_past_five_minutes() {
    let last_full_pass = tokio::time::Instant::now();
    let hard_deadline = last_full_pass + TICK;
    let mut deadline = hard_deadline;
    let mut healthy = true;

    for _ in 0..100 {
        let next_health = !healthy;
        deadline = deadline_after_health_change(deadline, last_full_pass, healthy, next_health);
        assert!(deadline <= hard_deadline);
        healthy = next_health;
        tokio::time::advance(Duration::from_secs(10)).await;
    }
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

    assert!(
        row_change_facets(&second.records, &second.changed, &previous).is_empty(),
        "an unchanged pass reports no row change"
    );
}

#[tokio::test]
async fn a_moved_cursor_reports_exactly_one_row_change_and_no_list_change() {
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

    let changes = row_change_facets(&second.records, &second.changed, &previous);
    assert_eq!(changes.len(), 1);
    let (session, facets) = &changes[0];
    assert_eq!(session.session_id, "moving");
    assert!(facets.metadata, "an append moves the activity metadata");
    assert!(!facets.title, "the title did not change");
}

#[tokio::test]
async fn a_new_session_reports_no_row_change_and_reports_a_list_change() {
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
    // `list_changed` refetch is what picks it up, not a row fact.
    assert!(row_change_facets(&described.records, &described.changed, &previous).is_empty());
}

#[test]
fn a_title_only_change_reports_the_title_facet_alone() {
    let previous = record_for_facets("stable", Some("Old title"), 1_000);
    let mut refreshed = previous.clone();
    refreshed.title = Some("New title".into());

    let facets = facets_between(&previous, &refreshed);

    assert!(facets.title);
    assert!(!facets.metadata);
    assert!(!facets.analysis);
}

#[test]
fn a_metadata_change_reports_the_metadata_facet_alone() {
    let previous = record_for_facets("stable", Some("Same title"), 1_000);
    let mut refreshed = previous.clone();
    refreshed.updated_at_epoch = Some(2_000);
    refreshed.activity_cursor = "moved".into();

    let facets = facets_between(&previous, &refreshed);

    assert!(!facets.title);
    assert!(facets.metadata);
}

/// A minimal stored record for the facet diff tests. Synthetic values only.
fn record_for_facets(session_id: &str, title: Option<&str>, at: i64) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new("native", "claude-code", session_id),
        source_kind: "file".into(),
        source_label: format!("/home/avery/.claude/projects/demo/{session_id}.jsonl"),
        wsl_distro: None,
        title: title.map(str::to_string),
        title_source: title.map(|_| "vendor".to_string()),
        cwd: Some("/home/avery/code/widgets".into()),
        surface: "cli".into(),
        updated_at_epoch: Some(at),
        activity_cursor: "cursor".into(),
        activity_source: "event".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: None,
    }
}

#[test]
fn a_reused_source_label_with_a_new_identity_still_reads_as_new() {
    let previous_owner = record_for_facets("first-owner", None, 1_000);
    let activity_key = SessionActivityKey::new(
        previous_owner.key.environment_key.clone(),
        previous_owner.key.agent.clone(),
        previous_owner.source_label.clone(),
    );
    let previous_records =
        std::collections::HashMap::from([(activity_key, previous_owner.clone())]);

    // A new session identity now writes to the same source label.
    let mut reused_label = record_for_facets("second-owner", None, 2_000);
    reused_label.source_label = previous_owner.source_label.clone();

    let sessions = indexed_sessions_for_report(
        2_010,
        std::slice::from_ref(&reused_label),
        &[reused_label.key.clone()],
        &previous_records,
        &[(reused_label.key.clone(), crate::store::Incarnation(7))],
    );

    assert_eq!(sessions.len(), 1);
    assert!(
        sessions[0].is_new,
        "a new identity is new even when its source label was known"
    );
    assert_eq!(
        sessions[0].incarnation,
        crate::store::Incarnation(7),
        "the fact carries the upsert's incarnation"
    );

    // A record the upsert returned no incarnation for was not written.
    let sessions = indexed_sessions_for_report(
        2_010,
        std::slice::from_ref(&reused_label),
        &[reused_label.key.clone()],
        &previous_records,
        &[],
    );
    assert!(sessions.is_empty());

    // The same identity again is not new, whatever its label.
    let known_key = SessionActivityKey::new("native", "claude-code", "/another/label.jsonl");
    let mut known = previous_owner.clone();
    known.updated_at_epoch = Some(2_000);
    let previous_records = std::collections::HashMap::from([(known_key, previous_owner)]);
    let sessions = indexed_sessions_for_report(
        2_010,
        std::slice::from_ref(&known),
        &[known.key.clone()],
        &previous_records,
        &[(known.key.clone(), crate::store::Incarnation(1))],
    );
    assert_eq!(sessions.len(), 1);
    assert!(!sessions[0].is_new);
}

/// The targeted (watcher) refresh path feeds `describe_with_states` a
/// previous map for the same labels. A label whose file now carries a new
/// session identity must read as a membership change, which
/// `refresh_sessions_locked` reports as `IndexChanged { ScanPass }`.
#[tokio::test]
async fn a_reused_source_label_reports_a_list_change_for_the_new_identity() {
    let home = tempfile::TempDir::new().unwrap();
    let path = write_claude_session(home.path(), "second-identity");
    let mut previous = record_for_facets("first-identity", None, 1_799_000_000);
    previous.source_label = path.to_string_lossy().into_owned();
    let previous_map = std::collections::HashMap::from([(
        SessionActivityKey::new("native", "claude-code", &previous.source_label),
        previous,
    )]);

    let described = describe_with_states(
        vec![log(AgentKind::Claude, path, 1_800_000_000)],
        home.path(),
        &HashSet::new(),
        &previous_map,
    )
    .await;

    assert_eq!(described.changed.len(), 1);
    assert_eq!(described.changed[0].session_id, "second-identity");
    assert!(
        described.list_changed,
        "a new identity behind a known label is a membership change"
    );
    // No previously known row to patch: the list refetch is the path.
    assert!(row_change_facets(&described.records, &described.changed, &previous_map).is_empty());
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

fn activity_key(agent: AgentKind, source_label: &str) -> SessionActivityKey {
    SessionActivityKey::new("native", agent.slug(), source_label)
}

fn agents(kinds: &[AgentKind]) -> BTreeSet<AgentKind> {
    kinds.iter().copied().collect()
}

#[test]
fn anonymous_generations_increase_and_the_ledger_keeps_the_highest_per_agent() {
    let mut ledger = AnonymousLedger::default();

    assert_eq!(ledger.issue(AgentKind::Codex), Some(AnonymousGen(1)));
    assert_eq!(ledger.issue(AgentKind::Claude), Some(AnonymousGen(2)));
    // A same-second second touch of the same agent still gets its own,
    // higher generation: causality, not time, orders it.
    assert_eq!(ledger.issue(AgentKind::Codex), Some(AnonymousGen(3)));

    assert_eq!(
        ledger.outstanding,
        BTreeMap::from([
            (AgentKind::Codex, AnonymousGen(3)),
            (AgentKind::Claude, AnonymousGen(2)),
        ])
    );
}

#[test]
fn an_exhausted_generation_counter_issues_nothing_rather_than_wrapping() {
    let mut ledger = AnonymousLedger {
        next: u64::MAX,
        outstanding: BTreeMap::new(),
    };
    assert_eq!(ledger.issue(AgentKind::Codex), None);
    assert_eq!(ledger.next, u64::MAX);
    assert!(ledger.outstanding.is_empty());

    // The touch that found the counter exhausted is not reported.
    let work = scoped::ScopedWork {
        agents: agents(&[AgentKind::Codex]),
        ..Default::default()
    };
    assert!(touch_observations(&mut ledger, &work, None, 1_000).is_empty());
}

#[test]
fn a_full_pass_captures_every_outstanding_agent_and_a_scoped_pass_only_its_own() {
    let mut ledger = AnonymousLedger::default();
    ledger.issue(AgentKind::Codex);
    ledger.issue(AgentKind::Claude);
    ledger.issue(AgentKind::Codex);

    assert_eq!(
        ledger.capture(&PassScope::Full),
        vec![
            AnonymousCover {
                agent: AgentKind::Claude,
                through: AnonymousGen(2),
            },
            AnonymousCover {
                agent: AgentKind::Codex,
                through: AnonymousGen(3),
            },
        ]
    );
    assert_eq!(
        ledger.capture(&PassScope::Agents(agents(&[AgentKind::Codex]))),
        vec![AnonymousCover {
            agent: AgentKind::Codex,
            through: AnonymousGen(3),
        }]
    );
    // A scope with nothing outstanding captures nothing, so a successful
    // pass over it sends no cover at all.
    assert!(
        ledger
            .capture(&PassScope::Agents(agents(&[AgentKind::Kiro])))
            .is_empty()
    );
}

#[test]
fn settling_forgets_only_generations_the_cover_reaches() {
    let mut ledger = AnonymousLedger::default();
    ledger.issue(AgentKind::Codex);
    ledger.issue(AgentKind::Claude);
    let captured = ledger.capture(&PassScope::Full);

    // A touch reported while the pass runs outranks the capture.
    ledger.issue(AgentKind::Codex);
    ledger.settle(&captured);

    assert_eq!(
        ledger.outstanding,
        BTreeMap::from([(AgentKind::Codex, AnonymousGen(3))]),
        "Claude settled at 2; Codex moved to 3 after the capture and stays"
    );

    // The next pass captures generation 3 and settles it.
    let captured = ledger.capture(&PassScope::Full);
    ledger.settle(&captured);
    assert!(ledger.outstanding.is_empty());

    // Settling a cover with nothing outstanding changes nothing.
    ledger.settle(&captured);
    assert!(ledger.outstanding.is_empty());
}

#[test]
fn a_failed_or_busy_pass_leaves_the_capture_outstanding() {
    let mut ledger = AnonymousLedger::default();
    ledger.issue(AgentKind::Codex);
    let captured = ledger.capture(&PassScope::Full);
    assert_eq!(captured.len(), 1);

    // Without a settle (the pass failed, was busy, or was cancelled), the
    // generation is still outstanding, and the next capture names it again.
    assert_eq!(ledger.capture(&PassScope::Full), captured);

    let ok = ScanStatus::default();
    assert!(pass_covers(&ok));
    let failed = ScanStatus {
        error: Some("disk full".into()),
        ..ScanStatus::default()
    };
    assert!(!pass_covers(&failed));
    let cancelled = ScanStatus {
        cancelled: true,
        ..ScanStatus::default()
    };
    assert!(!pass_covers(&cancelled));
}

#[test]
fn a_burst_reports_keyed_touches_then_one_generation_per_agent_lane() {
    let mut ledger = AnonymousLedger::default();
    let known = activity_key(
        AgentKind::Claude,
        "/home/avery/.claude/projects/p/known.jsonl",
    );
    let deleted = activity_key(
        AgentKind::Claude,
        "/home/avery/.claude/projects/p/gone.jsonl",
    );
    let work = scoped::ScopedWork {
        sessions: BTreeSet::from([known.clone(), deleted]),
        agents: agents(&[AgentKind::Codex]),
        db_agents: agents(&[AgentKind::Cursor]),
        quiet_agents: agents(&[AgentKind::Kiro]),
        title_agents: agents(&[AgentKind::Claude]),
    };
    let identities = std::collections::HashMap::from([(
        known,
        (
            SessionKey::new("native", "claude-code", "known-id"),
            crate::store::Incarnation(4),
        ),
    )]);

    let facts = touch_observations(
        &mut ledger,
        &work,
        Some((identities, crate::store::Revision(9))),
        1_000,
    );

    assert_eq!(
        facts,
        vec![
            session_lifecycle::Observation::Touched {
                session: session_lifecycle::TouchedSession {
                    key: SessionKey::new("native", "claude-code", "known-id"),
                    incarnation: crate::store::Incarnation(4),
                    seen: crate::store::Revision(9),
                },
                agent: AgentKind::Claude,
                at: 1_000,
            },
            session_lifecycle::Observation::Anonymous {
                agent: AgentKind::Codex,
                at: 1_000,
                generation: AnonymousGen(1),
            },
            session_lifecycle::Observation::Anonymous {
                agent: AgentKind::Cursor,
                at: 1_000,
                generation: AnonymousGen(2),
            },
        ],
        "the deleted key has no anonymous substitute; quiet and title lanes report nothing"
    );
    assert_eq!(
        ledger.outstanding,
        BTreeMap::from([
            (AgentKind::Codex, AnonymousGen(1)),
            (AgentKind::Cursor, AnonymousGen(2)),
        ])
    );
}

#[test]
fn a_failed_identity_lookup_reports_no_keyed_touch_and_no_substitute() {
    let mut ledger = AnonymousLedger::default();
    let work = scoped::ScopedWork {
        sessions: BTreeSet::from([activity_key(
            AgentKind::Claude,
            "/home/avery/.claude/x.jsonl",
        )]),
        agents: agents(&[AgentKind::Codex]),
        ..Default::default()
    };

    let facts = touch_observations(&mut ledger, &work, None, 1_000);

    assert_eq!(
        facts,
        vec![session_lifecycle::Observation::Anonymous {
            agent: AgentKind::Codex,
            at: 1_000,
            generation: AnonymousGen(1),
        }]
    );
}

#[test]
fn a_quiet_only_burst_issues_no_generation_and_its_pass_covers_nothing() {
    let mut ledger = AnonymousLedger::default();
    let work = scoped::ScopedWork {
        quiet_agents: agents(&[AgentKind::Claude]),
        ..Default::default()
    };

    assert!(touch_observations(&mut ledger, &work, None, 1_000).is_empty());
    assert!(ledger.outstanding.is_empty());
    assert!(
        ledger
            .capture(&PassScope::Agents(agents(&[AgentKind::Claude])))
            .is_empty()
    );

    // An outstanding generation for the same agent from an earlier
    // anonymous touch is covered by the quiet rediscovery, since that pass
    // discovers everything under the agent's root.
    ledger.issue(AgentKind::Claude);
    assert_eq!(
        ledger.capture(&PassScope::Agents(agents(&[AgentKind::Claude]))),
        vec![AnonymousCover {
            agent: AgentKind::Claude,
            through: AnonymousGen(1),
        }]
    );
}

/// The scheduler wiring cannot run without a Tauri app. Its shape is pinned
/// at the source instead: every scheduler-owned pass captures before it
/// runs, settles and covers only on success, and the cover is the last
/// report of the pass on the waiting path. A pass a command asks for goes
/// through `run_pass`, which owns no ledger and covers nothing.
#[test]
fn only_scheduler_owned_passes_cover_and_only_after_their_indexed_reports() {
    let source = include_str!("mod.rs").replace("\r\n", "\n");
    for checkout in [source.clone(), source.replace('\n', "\r\n")] {
        assert_scheduler_source_contract(&checkout);
    }
}

fn assert_scheduler_source_contract(source: &str) {
    let source = source.replace("\r\n", "\n");
    let production = source.split("#[cfg(test)]").next().unwrap_or(&source);

    let covered = {
        let start = production.find("async fn run_covered_pass(").unwrap();
        let body = &production[start..];
        &body[..body.find("\n}\n").unwrap()]
    };
    let capture = covered
        .find("ledger.capture(&scope)")
        .expect("captures at start");
    let run = covered
        .find("try_run_pass(app, None, trigger, scope).await?")
        .expect("runs the pass");
    let gate = covered
        .find("if pass_covers(&status) {")
        .expect("gates on success");
    let settle = covered.find("ledger.settle(&covers);").expect("settles");
    let report = covered
        .find("report_covered(app, covers).await;")
        .expect("covers");
    assert!(capture < run && run < gate && gate < settle && settle < report);

    // Every scheduler pass goes through the covered helper; nothing else
    // sends a cover.
    assert_eq!(
        production.matches("run_covered_pass(").count(),
        4,
        "one definition, three call sites"
    );
    assert!(
        !production.contains("run_pass(&app, None, "),
        "the scheduler runs no uncovered pass"
    );
    assert_eq!(
        production.matches("report_covered(").count(),
        2,
        "one definition, one call site"
    );
    assert_eq!(
        production.matches("Observation::AnonymousCovered").count(),
        1
    );
    assert!(!include_str!("scoped.rs").contains("AnonymousCovered"));
    assert!(!include_str!("../commands.rs").contains("AnonymousCovered"));

    // Anonymous touches are issued only from the burst path, before any
    // floor or pass, so a generation issued during a pass is above its
    // capture.
    assert_eq!(production.matches("ledger.issue(").count(), 1);
    assert_eq!(production.matches("Observation::Anonymous {").count(), 1);
}

mod producers;
