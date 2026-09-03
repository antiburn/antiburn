use super::*;

/// A row store for a test pass that wants published evidence. A pass
/// without a row store publishes no evidence, so any test that reads
/// `session.evidence` or `pass.evidence` needs one of these.
fn turn_row_store(agent: &str, session_id: &str) -> Arc<dyn TurnRowStore> {
    MemoryTurnRowStore::new(agent, session_id)
}

/// Reads the value out of an `EvidenceValue`, for `Complete` and
/// `Partial` alike. Panics on `Unsupported` — every group these tests
/// read is supported by the Claude capability set.
fn observed<T: Clone>(value: &EvidenceValue<T>) -> T {
    match value {
        EvidenceValue::Complete(observed) | EvidenceValue::Partial { observed, .. } => {
            observed.clone()
        }
        EvidenceValue::Unsupported => panic!("evidence group must be supported"),
    }
}

fn member_with_start(subagent_id: &str, started_at_epoch: Option<i64>) -> SubagentMember {
    SubagentMember {
        agent: "claude-code".to_string(),
        subagent_id: subagent_id.to_string(),
        label: "Sub-agent".to_string(),
        cost: None,
        tokens: None,
        model_runs: Vec::new(),
        started_at_epoch,
    }
}

#[test]
fn sort_members_orders_earliest_first_and_puts_unknown_starts_last() {
    let mut members = vec![
        member_with_start("late", Some(200)),
        member_with_start("unknown-first", None),
        member_with_start("early", Some(100)),
        member_with_start("unknown-second", None),
    ];

    sort_members(&mut members);

    let ids: Vec<&str> = members
        .iter()
        .map(|member| member.subagent_id.as_str())
        .collect();
    // Timed members sort earliest-first; both `None` members follow,
    // keeping their original relative order (a stable sort).
    assert_eq!(
        ids,
        vec!["early", "late", "unknown-first", "unknown-second"]
    );
}

fn claude_record(id: &str, timestamp: i64) -> String {
    format!(
        concat!(
            r#"{{"type":"assistant","timestamp":{timestamp},"message":{{"id":"{id}","role":"assistant","model":"claude-3-5-haiku-20241022","usage":{{"input_tokens":2,"output_tokens":3}},"content":[{{"type":"text","text":"Synthetic output."}}]}}}}"#,
            "\n"
        ),
        timestamp = timestamp,
        id = id,
    )
}

/// Like [`claude_record`], with an explicit model and an optional
/// top-level `speed` signal.
fn claude_record_with(id: &str, timestamp: i64, model: &str, speed: Option<&str>) -> String {
    let speed_field = speed
        .map(|speed| format!(",\"speed\":\"{speed}\""))
        .unwrap_or_default();
    format!(
        "{{\"type\":\"assistant\",\"timestamp\":{timestamp}{speed_field},\"message\":{{\"id\":\"{id}\",\"role\":\"assistant\",\"model\":\"{model}\",\"usage\":{{\"input_tokens\":2,\"output_tokens\":3}},\"content\":[{{\"type\":\"text\",\"text\":\"Synthetic output.\"}}]}}}}\n"
    )
}

fn file_input(path: &std::path::Path, id: &str) -> SessionInput {
    SessionInput {
        agent: "claude".to_string(),
        session_id: id.to_string(),
        source: RawSource::File(path.to_path_buf()),
    }
}

fn inline_input(content: String, id: &str) -> SessionInput {
    SessionInput {
        agent: "claude".to_string(),
        session_id: id.to_string(),
        source: RawSource::Jsonl(content),
    }
}

fn opencode_database() -> (tempfile::TempDir, std::path::PathBuf, String) {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let path = directory.path().join("opencode.db");
    let connection = rusqlite::Connection::open(&path).expect("database");
    connection
        .execute_batch(
            r#"CREATE TABLE session (
                 id TEXT PRIMARY KEY, parent_id TEXT, title TEXT,
                 time_created INTEGER, time_updated INTEGER
             );
             CREATE TABLE message (
                 id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                 time_updated INTEGER, data TEXT
             );
             CREATE TABLE part (
                 id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
                 time_created INTEGER, time_updated INTEGER, data TEXT
             );
             INSERT INTO session (id, parent_id, time_created, time_updated)
             VALUES ('root', NULL, 100, 120);
             INSERT INTO message VALUES (
                 'message', 'root', 110, 110,
                 '{"role":"assistant","modelID":"model-a","tokens":{"input":12,"output":3}}'
             );"#,
        )
        .expect("OpenCode fixture");
    drop(connection);
    (directory, path, "sv1:db:120:2".to_owned())
}

fn antigravity_database() -> (tempfile::TempDir, std::path::PathBuf) {
    fn varint(mut value: u64, out: &mut Vec<u8>) {
        while value >= 0x80 {
            out.push((value as u8 & 0x7f) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }
    fn scalar(field: u64, value: u64, out: &mut Vec<u8>) {
        varint(field << 3, out);
        varint(value, out);
    }
    fn bytes(field: u64, value: &[u8], out: &mut Vec<u8>) {
        varint((field << 3) | 2, out);
        varint(value.len() as u64, out);
        out.extend_from_slice(value);
    }

    let mut usage = Vec::new();
    scalar(1, 777, &mut usage);
    scalar(2, 30, &mut usage);
    scalar(3, 50, &mut usage);
    scalar(4, 7, &mut usage);
    scalar(5, 800, &mut usage);
    scalar(9, 40, &mut usage);
    scalar(10, 10, &mut usage);
    bytes(11, b"response-1", &mut usage);
    let mut chat_model = Vec::new();
    bytes(4, &usage, &mut chat_model);
    bytes(19, b"gemini-3.6-flash", &mut chat_model);
    let mut blob = Vec::new();
    bytes(1, &chat_model, &mut blob);

    let directory = tempfile::TempDir::new().expect("tempdir");
    let subroot = directory.path().join("antigravity-cli");
    let conversations = subroot.join("conversations");
    let logs = subroot
        .join("brain")
        .join("root")
        .join(".system_generated")
        .join("logs");
    std::fs::create_dir_all(&conversations).unwrap();
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(
        logs.join("transcript.jsonl"),
        concat!(
            r#"{"type":"USER_INPUT","created_at":"2026-01-01T00:00:00Z","content":"hello"}"#,
            "\n",
            r#"{"type":"PLANNER_RESPONSE","created_at":"2026-01-01T00:00:01Z","content":"done"}"#,
            "\n"
        ),
    )
    .unwrap();
    let path = conversations.join("root.db");
    let connection = rusqlite::Connection::open(&path).expect("database");
    connection
        .execute_batch(
            "CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);
             CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB, size INTEGER NOT NULL DEFAULT 0);
             INSERT INTO steps(idx) VALUES (0), (1);",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO gen_metadata(idx, data, size) VALUES (0, ?1, ?2)",
            rusqlite::params![blob, blob.len() as i64],
        )
        .unwrap();
    drop(connection);
    (directory, path)
}

#[tokio::test]
async fn an_opencode_provider_database_stays_native() {
    let (_directory, path, _) = opencode_database();
    let source = SessionSource::ProviderDb {
        agent: AgentKind::OpenCode,
        db_path: path.clone(),
        session_id: "root".to_owned(),
    };

    assert_eq!(raw_source(&source).await, Some(RawSource::Sqlite(path)));
}

#[tokio::test]
async fn a_claimed_antigravity_database_stays_native_and_publishes() {
    let (_directory, path) = antigravity_database();
    let source = SessionSource::ProviderDb {
        agent: AgentKind::Antigravity,
        db_path: path.clone(),
        session_id: "root".to_owned(),
    };
    assert_eq!(
        raw_source(&source).await,
        Some(RawSource::Sqlite(path.clone()))
    );
    let (latest, rows) = Explorers::DISK
        .provider_db_fingerprint(&AgentKind::Antigravity, &path, "root")
        .await
        .expect("database fingerprint");
    let fingerprint = format!("sv1:db:{latest}:{rows}");
    let input = SessionInput {
        agent: "antigravity".to_owned(),
        session_id: "root".to_owned(),
        source: RawSource::Sqlite(path),
    };

    let outcome = stream_vendor_with_hooks(
        &[input],
        &|| false,
        &|_, _| {},
        Some(&fingerprint),
        Some(turn_row_store("antigravity", "root")),
    );

    let StreamOutcome::Published {
        session,
        parent_fingerprint,
    } = outcome
    else {
        panic!("a stable Antigravity database must publish");
    };
    assert_eq!(parent_fingerprint.as_deref(), Some(fingerprint.as_str()));
    assert_eq!(session.parent.billable_input_tokens, 30);
    assert_eq!(session.parent.billable_output_tokens, 50);
    assert_eq!(session.parent.peak_context_tokens, 837);
    assert_eq!(session.parent.billable_cache_creation_tokens, 7);
    let evidence = session.evidence.expect("database evidence");
    assert_eq!(evidence.provenance.parser_revision, PARSER_REVISION);
    assert!(evidence.capabilities.cache_write_tokens);
    assert!(evidence.capabilities.token_classes);
    assert_eq!(observed(&evidence.context).max_request_context_tokens, 837);
    assert_eq!(observed(&evidence.cache).cache_creation_tokens, 7);
    assert!(!observed(&evidence.models).by_model.is_empty());
}

#[test]
fn a_claimed_opencode_database_publishes_from_the_validated_snapshot() {
    let (_directory, path, fingerprint) = opencode_database();
    let input = SessionInput {
        agent: "opencode".to_owned(),
        session_id: "root".to_owned(),
        source: RawSource::Sqlite(path),
    };

    let outcome = stream_vendor_with_hooks(
        &[input],
        &|| false,
        &|_, _| {},
        Some(&fingerprint),
        Some(turn_row_store("opencode", "root")),
    );

    let StreamOutcome::Published {
        session,
        parent_fingerprint,
    } = outcome
    else {
        panic!("a stable OpenCode database must publish");
    };
    assert_eq!(parent_fingerprint.as_deref(), Some(fingerprint.as_str()));
    assert_eq!(session.parent.billable_input_tokens, 12);
    assert_eq!(session.parent.billable_output_tokens, 3);
    assert!(session.evidence.is_some());
}

/// Like [`opencode_database`], with one `parent_id` child session
/// carrying one assistant message on `model`. A separate helper (rather
/// than a parameter on `opencode_database`) keeps that helper's own
/// fingerprint assertions stable.
fn opencode_database_with_delegated_child(model: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let path = directory.path().join("opencode.db");
    let connection = rusqlite::Connection::open(&path).expect("database");
    connection
        .execute_batch(
            r#"CREATE TABLE session (
                 id TEXT PRIMARY KEY, parent_id TEXT, title TEXT,
                 time_created INTEGER, time_updated INTEGER
             );
             CREATE TABLE message (
                 id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                 time_updated INTEGER, data TEXT
             );
             CREATE TABLE part (
                 id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
                 time_created INTEGER, time_updated INTEGER, data TEXT
             );
             INSERT INTO session (id, parent_id, time_created, time_updated)
             VALUES ('root', NULL, 100, 120);
             INSERT INTO session (id, parent_id, time_created, time_updated)
             VALUES ('child', 'root', 110, 115);
             INSERT INTO message VALUES (
                 'message', 'root', 100, 100,
                 '{"role":"assistant","modelID":"model-a","tokens":{"input":12,"output":3}}'
             );"#,
        )
        .expect("OpenCode fixture");
    connection
        .execute(
            "INSERT INTO message VALUES ('child-message', 'child', 110, 110, ?1)",
            [format!(
                r#"{{"role":"assistant","modelID":"{model}","tokens":{{"input":4,"output":1}}}}"#
            )],
        )
        .expect("child message");
    drop(connection);
    (directory, path)
}

#[test]
fn an_opencode_parent_id_child_links_as_a_delegated_thread() {
    let (_directory, path) = opencode_database_with_delegated_child("model-b");
    let input = SessionInput {
        agent: "opencode".to_owned(),
        session_id: "root".to_owned(),
        source: RawSource::Sqlite(path),
    };

    let pass = evidence_pass_with_turn_rows(
        &[input],
        &|| false,
        Some(turn_row_store("opencode", "root")),
    );
    let evidence = pass.evidence.expect("published evidence");

    assert!(matches!(evidence.subagents, EvidenceValue::Complete(_)));
    let subagents = observed(&evidence.subagents);
    assert_eq!(subagents.spawn_count, 1);
    assert!(subagents.delegated_models.contains("model-b"));
    assert_eq!(subagents.delegated_turns, 1);
}

fn codex_record() -> String {
    [
        r#"{"timestamp":"2026-08-01T09:59:58Z","type":"session_meta","payload":{"id":"synthetic","timestamp":"2026-08-01T09:59:58Z","source":"cli"}}"#,
        r#"{"timestamp":"2026-08-01T10:00:00Z","type":"turn_context","payload":{"model":"gpt-test","effort":"medium"}}"#,
        r#"{"timestamp":"2026-08-01T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":2,"total_tokens":112},"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":2,"total_tokens":112},"model_context_window":200000}}}"#,
    ]
    .join("\n")
        + "\n"
}

fn codex_file_input(path: &std::path::Path, id: &str) -> SessionInput {
    SessionInput {
        agent: "codex".to_string(),
        session_id: id.to_string(),
        source: RawSource::File(path.to_path_buf()),
    }
}

fn pi_record() -> String {
    [
        r#"{"type":"session","version":3,"timestamp":"2026-08-01T09:59:58Z"}"#,
        r#"{"type":"thinking_level_change","timestamp":"2026-08-01T10:00:00Z","thinkingLevel":"medium"}"#,
        r#"{"type":"message","timestamp":"2026-08-01T10:00:01Z","message":{"role":"assistant","api":"anthropic-messages","provider":"anthropic","model":"model-a","usage":{"input":2,"output":3,"cacheRead":5,"cacheWrite":7},"content":[]}}"#,
    ]
    .join("\n")
        + "\n"
}

#[test]
fn an_accepted_claude_read_publishes_metrics_and_a_start_time() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let path = directory.path().join("parent.jsonl");
    std::fs::write(&path, claude_record("parent", 1_760_000_000)).expect("write parent");

    let StreamOutcome::Published { session, .. } =
        stream_vendor(&[file_input(&path, "parent")], &CancelFlag::never())
    else {
        panic!("stable source must publish");
    };
    assert_eq!(session.parent.event_count, 1);
    assert_eq!(session.parent.tokens_in, 2);
    assert_eq!(session.parent.tokens_out, 3);
    assert_eq!(session.started_at_epoch, Some(1_760_000_000));
}

#[test]
fn codex_read_publishes_its_capabilities_and_provider_start() {
    let input = SessionInput {
        agent: "codex".to_owned(),
        session_id: "codex-inline".to_owned(),
        source: RawSource::Jsonl(codex_record()),
    };

    let StreamOutcome::Published { session, .. } =
        stream_vendor(std::slice::from_ref(&input), &CancelFlag::never())
    else {
        panic!("Codex source must publish");
    };
    assert_eq!(session.started_at_epoch, Some(1_785_578_398));
    // `stream_vendor` attaches no row store, so this pass alone
    // publishes no evidence; the row-backed pass below does.
    assert!(session.evidence.is_none());

    let pass = evidence_pass_with_turn_rows(
        &[input],
        &|| false,
        Some(turn_row_store("codex", "codex-inline")),
    );
    assert_eq!(pass.outcome, PassOutcome::Published);
    // `cache_write_tokens` is observed per session: `codex_record`
    // carries no cache-write alias key, so it reads false even though
    // `SourceCapabilities::codex()` now defaults it true.
    let mut expected_capabilities = SourceCapabilities::codex();
    expected_capabilities.cache_write_tokens = false;
    assert_eq!(pass.evidence.unwrap().capabilities, expected_capabilities);
}

#[test]
fn pi_read_publishes_through_the_evidence_path() {
    let input = SessionInput {
        agent: "pi".to_owned(),
        session_id: "pi-inline".to_owned(),
        source: RawSource::Jsonl(pi_record()),
    };

    let StreamOutcome::Published { session, .. } =
        stream_vendor(std::slice::from_ref(&input), &CancelFlag::never())
    else {
        panic!("Pi source must publish");
    };
    assert_eq!(session.started_at_epoch, Some(1_785_578_398));
    assert_eq!(session.parent.peak_context_tokens, 14);
    // `stream_vendor` attaches no row store, so this pass alone
    // publishes no evidence; the row-backed pass below does.
    assert!(session.evidence.is_none());

    let pass =
        evidence_pass_with_turn_rows(&[input], &|| false, Some(turn_row_store("pi", "pi-inline")));
    assert_eq!(pass.outcome, PassOutcome::Published);
    let record = pass
        .analysis
        .record(&SessionKey::new("native", "pi", "pi-inline"))
        .expect("Pi analysis record");
    assert_eq!(
        record.provider_hints_json.as_deref(),
        Some(r#"[{"provider":"anthropic","model":"model-a"}]"#)
    );
    assert_eq!(
        pass.evidence.unwrap().capabilities,
        SourceCapabilities::pi()
    );
}

/// Seam 4f: a Pi file whose messages chain through a `model_change`
/// stays one thread, so `cache` publishes `Complete` and the
/// over-depth check becomes assessable — the capability the Pi
/// `thread_identity` flip is for.
#[test]
fn pi_thread_chain_through_a_model_change_supports_cache_and_overdepth() {
    let input = SessionInput {
        agent: "pi".to_owned(),
        session_id: "pi-thread-chain".to_owned(),
        source: RawSource::Jsonl(
            [
                r#"{"type":"session","version":3,"timestamp":"2026-08-01T09:59:58Z"}"#,
                r#"{"type":"message","id":"pi-thread-1","parentId":null,"timestamp":"2026-08-01T10:00:00Z","message":{"role":"assistant","model":"model-a","usage":{"input":2,"output":3,"cacheRead":5,"cacheWrite":7},"content":[]}}"#,
                r#"{"type":"model_change","id":"pi-thread-2","parentId":"pi-thread-1","timestamp":"2026-08-01T10:00:01Z","modelId":"model-b"}"#,
                r#"{"type":"message","id":"pi-thread-3","parentId":"pi-thread-2","timestamp":"2026-08-01T10:00:02Z","message":{"role":"assistant","model":"model-b","usage":{"input":3,"output":4,"cacheRead":1,"cacheWrite":0},"content":[]}}"#,
            ]
            .join("\n")
                + "\n",
        ),
    };

    let pass = evidence_pass_with_turn_rows(
        &[input],
        &|| false,
        Some(turn_row_store("pi", "pi-thread-chain")),
    );
    assert_eq!(pass.outcome, PassOutcome::Published);
    let evidence = pass.evidence.expect("published evidence");

    assert!(evidence.capabilities.thread_identity);
    assert!(matches!(evidence.cache, EvidenceValue::Complete(_)));
    let cache = observed(&evidence.cache);
    assert_eq!(
        cache.model_transitions.len(),
        1,
        "the model_change between the two messages must count as one transition on their shared thread"
    );

    assert!(
        eligible(DetectorId::SessionsOverDepth, &evidence),
        "SessionsOverDepth must be eligible once thread_identity is set"
    );
}

/// Seam 5a: a Codex rollout is one thread, but its records carry no
/// per-record id. `thread_identity` stays set and unblocks
/// SessionsOverDepth; `record_identity` stays unset, but Codex's own
/// `linear_record_order` (one rollout, one append-only stream) attests
/// `RecordLinkage` from line order instead, so CacheChurn can read
/// clean once more.
#[test]
fn codex_thread_identity_without_record_identity_still_attests_linkage_from_order_for_cache_churn()
{
    let input = SessionInput {
        agent: "codex".to_owned(),
        session_id: "codex-thread-identity".to_owned(),
        source: RawSource::Jsonl(codex_record()),
    };

    let pass = evidence_pass_with_turn_rows(
        &[input],
        &|| false,
        Some(turn_row_store("codex", "codex-thread-identity")),
    );
    assert_eq!(pass.outcome, PassOutcome::Published);
    let evidence = pass.evidence.expect("published evidence");

    assert!(evidence.capabilities.thread_identity);
    assert!(!evidence.capabilities.record_identity);
    assert!(evidence.capabilities.linear_record_order);

    assert!(
        eligible(DetectorId::SessionsOverDepth, &evidence),
        "SessionsOverDepth must be eligible for Codex"
    );
    // Codex reports `token_classes` and `request_context_tokens`.
    // `evidence_sink` pins Codex to uncached-input accounting for
    // `repeated_context` regardless of `cache_write_tokens`.
    // This makes Cache Churn eligible. This fixture has no record loss.
    // The order route makes `RecordLinkage` complete, so Cache Churn reads clean.
    assert!(
        eligible(DetectorId::CacheChurn, &evidence),
        "CacheChurn must be eligible for Codex under uncached-input accounting"
    );
    assert!(
        clean_facts_complete(DetectorId::CacheChurn, &evidence),
        "CacheChurn must read clean for Codex once linear record order attests linkage"
    );
}

#[test]
fn a_changed_codex_source_publishes_neither_projection() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let path = directory.path().join("codex.jsonl");
    std::fs::write(&path, codex_record()).expect("write Codex source");
    let hook = |_: usize, path: &std::path::Path| {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .expect("open Codex source")
            .write_all(b"{}\n")
            .expect("append Codex source");
    };

    assert!(matches!(
        stream_vendor_with_claim_hook(
            &[codex_file_input(&path, "codex")],
            &CancelFlag::never(),
            &hook,
        ),
        StreamOutcome::SourceChanged
    ));
}

#[test]
fn an_accepted_child_read_publishes_the_merged_metrics() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let parent = directory.path().join("parent.jsonl");
    let child = directory.path().join("child.jsonl");
    std::fs::write(&parent, claude_record("parent", 1_760_000_000)).expect("write parent");
    std::fs::write(&child, claude_record("child", 1_760_000_001)).expect("write child");
    let inputs = [file_input(&parent, "parent"), file_input(&child, "child")];

    let StreamOutcome::Published { session, .. } = stream_vendor(&inputs, &CancelFlag::never())
    else {
        panic!("stable sources must publish");
    };
    assert_eq!(session.subagents.len(), 1);
    assert_eq!(session.merged.event_count, 2);
    assert_eq!(session.merged.tokens_in, 4);
    assert_eq!(session.merged.tokens_out, 6);
}

#[test]
fn a_changed_parent_source_publishes_neither_projection() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let path = directory.path().join("parent.jsonl");
    std::fs::write(&path, claude_record("parent", 1_760_000_000)).expect("write parent");
    let hook = |_: usize, path: &std::path::Path| {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .expect("open parent")
            .write_all(claude_record("later", 1_760_000_001).as_bytes())
            .expect("append parent");
    };

    assert!(matches!(
        stream_vendor_with_claim_hook(&[file_input(&path, "parent")], &CancelFlag::never(), &hook,),
        StreamOutcome::SourceChanged
    ));
}

#[test]
fn a_changed_child_source_publishes_neither_projection() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let parent = directory.path().join("parent.jsonl");
    let child = directory.path().join("child.jsonl");
    std::fs::write(&parent, claude_record("parent", 1_760_000_000)).expect("write parent");
    std::fs::write(&child, claude_record("child", 1_760_000_001)).expect("write child");
    let hook = |index: usize, path: &std::path::Path| {
        if index == 1 {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .expect("open child")
                .write_all(claude_record("later", 1_760_000_002).as_bytes())
                .expect("append child");
        }
    };

    assert!(matches!(
        stream_vendor_with_claim_hook(
            &[file_input(&parent, "parent"), file_input(&child, "child")],
            &CancelFlag::never(),
            &hook,
        ),
        StreamOutcome::SourceChanged
    ));
}

#[test]
fn a_missing_child_is_skipped_and_the_session_still_publishes() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let parent = directory.path().join("parent.jsonl");
    std::fs::write(&parent, claude_record("parent", 1_760_000_000)).expect("write parent");
    let missing = directory.path().join("missing.jsonl");

    let StreamOutcome::Published { session, .. } = stream_vendor(
        &[file_input(&parent, "parent"), file_input(&missing, "child")],
        &CancelFlag::never(),
    ) else {
        panic!("missing child must not block the parent");
    };
    assert!(session.subagents.is_empty());
    assert_eq!(session.parent.event_count, 1);
    assert_eq!(session.parent.tokens_in, 2);
}

#[cfg(unix)]
#[test]
fn an_unreadable_child_is_skipped_and_the_session_still_publishes() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::TempDir::new().expect("tempdir");
    let parent = directory.path().join("parent.jsonl");
    let unreadable = directory.path().join("unreadable.jsonl");
    let remaining = directory.path().join("remaining.jsonl");
    std::fs::write(&parent, claude_record("parent", 1_760_000_000)).expect("write parent");
    std::fs::write(&unreadable, claude_record("unreadable", 1_760_000_001))
        .expect("write unreadable child");
    std::fs::write(&remaining, claude_record("remaining", 1_760_000_002))
        .expect("write remaining child");
    let make_child_unreadable = |index: usize, path: &std::path::Path| {
        if index == 1 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))
                .expect("remove child read permission");
        }
    };

    let outcome = stream_vendor_with_claim_hook(
        &[
            file_input(&parent, "parent"),
            file_input(&unreadable, "unreadable"),
            file_input(&remaining, "remaining"),
        ],
        &CancelFlag::never(),
        &make_child_unreadable,
    );
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o600))
        .expect("restore child read permission");

    let StreamOutcome::Published { session, .. } = outcome else {
        panic!("unreadable child must not block readable sources");
    };
    assert_eq!(session.parent.session_id, "parent");
    assert_eq!(session.parent.event_count, 1);
    assert_eq!(session.subagents.len(), 1);
    assert_eq!(session.subagents[0].0.session_id, "remaining");
    assert_eq!(session.subagents[0].0.event_count, 1);
    assert_eq!(session.merged.event_count, 2);
}

#[test]
fn streaming_inline_metrics_equal_the_shipped_batch() {
    let input = inline_input(claude_record("inline-equality", 1_760_000_000), "inline");
    let expected = analyze_sources_with(vec![input.clone()], true)
        .sessions
        .into_iter()
        .next()
        .expect("batch metrics");

    let StreamOutcome::Published { session, .. } = stream_vendor(&[input], &CancelFlag::never())
    else {
        panic!("inline source must publish");
    };
    assert_eq!(session.parent, expected);
}

#[test]
fn an_inline_source_reports_unvalidated_and_publishes() {
    let input = inline_input(claude_record("inline-outcome", 1_760_000_000), "inline");
    let mut accumulator = SessionMetricsAccumulator::new("claude", "inline");

    assert_eq!(
        ClaudeAdapter
            .visit(&input, &mut accumulator)
            .expect("inline visit"),
        VisitOutcome::Unvalidated
    );
    assert!(matches!(
        stream_vendor(&[input], &CancelFlag::never()),
        StreamOutcome::Published { .. }
    ));
}

#[test]
fn an_inline_source_still_publishes_metrics() {
    let pass = evidence_pass(
        &[inline_input(
            claude_record("inline-metrics", 1_760_000_000),
            "inline-metrics",
        )],
        &|| false,
    );

    assert_eq!(pass.outcome, PassOutcome::Published);
    assert!(pass.analysis.metrics.is_some());
}

#[test]
fn a_published_claude_pass_carries_evidence() {
    let pass = evidence_pass_with_turn_rows(
        &[inline_input(
            claude_record("inline-evidence", 1_760_000_000),
            "inline-evidence",
        )],
        &|| false,
        Some(turn_row_store("claude", "inline-evidence")),
    );

    let evidence = pass.evidence.expect("published evidence");
    assert_eq!(pass.outcome, PassOutcome::Published);
    assert_eq!(evidence.schema_revision, EVIDENCE_SCHEMA_REVISION);
}

#[test]
fn a_child_only_fast_signal_reaches_the_parents_evidence() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let parent = directory.path().join("parent.jsonl");
    let child = directory.path().join("child.jsonl");
    std::fs::write(&parent, claude_record("fast-signal-parent", 1_760_000_000))
        .expect("write parent");
    std::fs::write(
        &child,
        claude_record_with(
            "fast-signal-child",
            1_760_000_001,
            "claude-opus-4-6",
            Some("fast"),
        ),
    )
    .expect("write child");

    let pass = evidence_pass_with_turn_rows(
        &[
            file_input(&parent, "fast-signal-parent"),
            file_input(&child, "fast-signal-child"),
        ],
        &|| false,
        Some(turn_row_store("claude", "fast-signal-parent")),
    );
    let evidence = pass.evidence.expect("published evidence");

    let models = observed(&evidence.models);
    let fast = models
        .fast_modes
        .get(FAST_SPEED_KEY)
        .expect("the child's fast signal must reach the parent's evidence");
    assert_eq!(fast.delegated, 1);
    assert_eq!(fast.main_loop, 0);

    let subagents = observed(&evidence.subagents);
    assert!(subagents.delegated_models.contains("claude-opus-4-6"));
}

/// A Codex parent rollout: one `turn_context` naming `model`, then one
/// `spawn_agent` function call that starts a subagent.
fn codex_spawn_parent_record(model: &str) -> String {
    format!(
        concat!(
            r#"{{"timestamp":"2026-08-12T10:00:00Z","type":"turn_context","payload":{{"model":"{model}","effort":"medium"}}}}"#,
            "\n",
            r#"{{"timestamp":"2026-08-12T10:00:01Z","type":"response_item","payload":{{"type":"function_call","name":"spawn_agent","arguments":"{{\"agent_type\":\"worker\"}}","call_id":"call-spawn"}}}}"#,
            "\n",
        ),
        model = model,
    )
}

/// A discovered Codex child rollout: `session_meta` marks it a subagent
/// replaying its parent's history, then the task addressed to the
/// child's agent path opens its owned usage window, which carries one
/// `turn_context` naming `model` and one assistant turn with usage.
fn codex_spawn_child_record(model: &str) -> String {
    format!(
        concat!(
            r#"{{"timestamp":"2026-08-12T10:00:02Z","type":"session_meta","payload":{{"id":"synthetic-spawn-child","thread_source":"subagent","agent_path":"worker","source":"cli"}}}}"#,
            "\n",
            r#"{{"timestamp":"2026-08-12T10:00:03Z","type":"event_msg","payload":{{"type":"task_started"}}}}"#,
            "\n",
            r#"{{"timestamp":"2026-08-12T10:00:04Z","type":"response_item","payload":{{"type":"agent_message","author":"parent","recipient":"worker","content":[{{"type":"input_text","text":"Handle the synthetic task."}}]}}}}"#,
            "\n",
            r#"{{"timestamp":"2026-08-12T10:00:05Z","type":"turn_context","payload":{{"model":"{model}","effort":"low"}}}}"#,
            "\n",
            r#"{{"timestamp":"2026-08-12T10:00:06Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":300,"cached_input_tokens":100,"output_tokens":40,"total_tokens":340}},"total_token_usage":{{"input_tokens":300,"cached_input_tokens":100,"output_tokens":40,"total_tokens":340}},"model_context_window":100000}}}}}}"#,
            "\n",
        ),
        model = model,
    )
}

#[test]
fn a_codex_spawn_agent_call_links_to_its_discovered_child() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let parent = directory.path().join("parent.jsonl");
    let child = directory.path().join("child.jsonl");
    std::fs::write(&parent, codex_spawn_parent_record("gpt-parent")).expect("write parent");
    std::fs::write(&child, codex_spawn_child_record("gpt-child")).expect("write child");

    let pass = evidence_pass_with_turn_rows(
        &[
            codex_file_input(&parent, "spawn-parent"),
            codex_file_input(&child, "spawn-child"),
        ],
        &|| false,
        Some(turn_row_store("codex", "spawn-parent")),
    );
    let evidence = pass.evidence.expect("published evidence");

    assert!(matches!(evidence.subagents, EvidenceValue::Complete(_)));
    let subagents = observed(&evidence.subagents);
    assert_eq!(subagents.spawn_count, 1);
    assert!(subagents.delegated_models.contains("gpt-child"));
    assert_eq!(subagents.delegated_turns, 1);
}

#[test]
fn a_model_switch_confined_to_one_child_produces_no_transition() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let parent = directory.path().join("parent.jsonl");
    let child = directory.path().join("child.jsonl");
    std::fs::write(&parent, claude_record("switch-parent", 1_760_000_000)).expect("write parent");
    std::fs::write(
        &child,
        claude_record_with("switch-child-1", 1_760_000_001, "model-a", None)
            + &claude_record_with("switch-child-2", 1_760_000_002, "model-b", None),
    )
    .expect("write child");

    let pass = evidence_pass_with_turn_rows(
        &[
            file_input(&parent, "switch-parent"),
            file_input(&child, "switch-child"),
        ],
        &|| false,
        Some(turn_row_store("claude", "switch-parent")),
    );
    let evidence = pass.evidence.expect("published evidence");

    let cache = observed(&evidence.cache);
    assert!(
        cache.model_transitions.is_empty(),
        "a model switch inside one child must not become a parent-thread transition"
    );
}

#[test]
fn an_unreadable_discovered_child_degrades_child_dependent_groups() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let parent = directory.path().join("parent.jsonl");
    std::fs::write(&parent, claude_record("unreadable-parent", 1_760_000_000))
        .expect("write parent");
    let missing_child = directory.path().join("missing-child.jsonl");

    let pass = evidence_pass_with_turn_rows(
        &[
            file_input(&parent, "unreadable-parent"),
            file_input(&missing_child, "unreadable-child"),
        ],
        &|| false,
        Some(turn_row_store("claude", "unreadable-parent")),
    );
    assert_eq!(pass.outcome, PassOutcome::Published);
    let evidence = pass.evidence.expect("published evidence");

    assert_eq!(evidence.diagnostics.children_unreadable, 1);
    assert!(matches!(
        evidence.subagents,
        EvidenceValue::Partial {
            reason: CoverageReason::ReadFailed,
            ..
        }
    ));
    assert!(matches!(
        evidence.models,
        EvidenceValue::Partial {
            reason: CoverageReason::ReadFailed,
            ..
        }
    ));
}

#[test]
fn two_children_with_different_models_share_no_transition_or_idle_gap() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let parent = directory.path().join("parent.jsonl");
    let first_child = directory.path().join("first-child.jsonl");
    let second_child = directory.path().join("second-child.jsonl");
    std::fs::write(&parent, claude_record("siblings-parent", 1_760_000_000)).expect("write parent");
    std::fs::write(
        &first_child,
        claude_record_with("siblings-child-1", 1_760_000_001, "model-a", None),
    )
    .expect("write first child");
    std::fs::write(
        &second_child,
        // Far enough past the first child's turn that a single shared
        // clock would read as a long idle gap.
        claude_record_with("siblings-child-2", 1_760_100_000, "model-b", None),
    )
    .expect("write second child");

    let pass = evidence_pass_with_turn_rows(
        &[
            file_input(&parent, "siblings-parent"),
            file_input(&first_child, "siblings-child-1"),
            file_input(&second_child, "siblings-child-2"),
        ],
        &|| false,
        Some(turn_row_store("claude", "siblings-parent")),
    );
    let evidence = pass.evidence.expect("published evidence");

    let cache = observed(&evidence.cache);
    assert!(
        cache.model_transitions.is_empty(),
        "two children never share a thread, so they form no transition"
    );
    assert_eq!(
        cache.longest_idle_gap_ms, 0,
        "two children never share a thread, so they form no idle gap"
    );
}

/// Pins the coverage record a full pass writes for a parent-plus-two-
/// children fixture, byte for byte, against the record the pre-refactor
/// in-place fold produced for the same fixture. Guards the residual
/// refactor (`ChildFold`): folding a clone at the end must still equal
/// folding into the parent as the loop goes.
#[test]
fn a_full_pass_writes_the_same_coverage_record_the_in_place_fold_did() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let parent = directory.path().join("parent.jsonl");
    let first_child = directory.path().join("first-child.jsonl");
    let second_child = directory.path().join("second-child.jsonl");
    std::fs::write(&parent, claude_record("fold-parity-parent", 1_760_000_000))
        .expect("write parent");
    std::fs::write(
        &first_child,
        claude_record_with("fold-parity-child-1", 1_760_000_001, "model-a", None),
    )
    .expect("write first child");
    std::fs::write(
        &second_child,
        claude_record_with("fold-parity-child-2", 1_760_000_002, "model-b", None),
    )
    .expect("write second child");

    let store = turn_row_store("claude", "fold-parity-parent");
    let pass = evidence_pass_with_turn_rows(
        &[
            file_input(&parent, "fold-parity-parent"),
            file_input(&first_child, "fold-parity-child-1"),
            file_input(&second_child, "fold-parity-child-2"),
        ],
        &|| false,
        Some(Arc::clone(&store)),
    );
    assert_eq!(pass.outcome, PassOutcome::Published);
    let record = store
        .query_coverage_record()
        .expect("coverage record query")
        .expect("coverage record");

    // Captured from the same fixture against the pre-refactor
    // in-place fold, before `ChildFold` existed.
    let before_refactor = r#"{"coverageSchemaRevision":1,"identity":{"agent":"claude","sessionId":"fold-parity-parent"},"capabilities":{"requestContextTokens":true,"cacheWriteTokens":true,"timestampsAndOrder":true,"toolInvocations":true,"skillMcpAttribution":true,"toolDefinitions":false,"modelIdentity":true,"tokenClasses":true,"reasoningEffortTier":true,"fastTier":true,"serviceTier":false,"subagentRelationships":true,"subagentModels":true,"compactionBoundaries":true,"threadIdentity":true,"recordIdentity":true,"linearRecordOrder":false,"quotaIncidents":false,"harnessVersion":false},"sourceKind":"file","sourceAcceptance":"accepted_full","ordering":"monotonic","diagnostics":{"recordsObserved":3,"recordsUnusable":0,"recordsUnrecognizedInert":0,"unusableReasons":{},"unrecognizedTypes":[],"truncatedStrings":[],"cappedCollections":[],"childrenDiscovered":2,"childrenUnreadable":0,"duplicateTurnIdentities":0},"recordLossReason":null,"sessionCapExceeded":false,"tools":{},"invokedSkills":[],"toolsCapExceeded":false,"skills":{},"mcpServers":{},"contextSourcesCapExceeded":false,"subagentSpawnCount":0,"subagentChildren":[],"subagentExamples":[],"subagentsCapExceeded":false,"threadParentUnresolved":false,"summaryObserved":true,"childLossReason":null}"#;
    assert_eq!(
        serde_json::to_string(&record).expect("encode"),
        before_refactor,
        "the residual refactor must not change the coverage record a full pass writes"
    );
}

/// A Claude assistant record with an explicit thread-identity chain
/// (`uuid` / `parentUuid`), optionally an inline sidechain.
fn claude_thread_record(
    uuid: &str,
    parent_uuid: Option<&str>,
    is_sidechain: bool,
    timestamp: i64,
    model: &str,
) -> String {
    let parent_uuid_field = parent_uuid
        .map(|parent| format!("\"{parent}\""))
        .unwrap_or_else(|| "null".to_owned());
    let sidechain_field = if is_sidechain {
        ",\"isSidechain\":true"
    } else {
        ""
    };
    format!(
        "{{\"type\":\"assistant\",\"uuid\":\"{uuid}\",\"parentUuid\":{parent_uuid_field}{sidechain_field},\"timestamp\":{timestamp},\"message\":{{\"id\":\"msg-{uuid}\",\"role\":\"assistant\",\"model\":\"{model}\",\"usage\":{{\"input_tokens\":2,\"output_tokens\":3}},\"content\":[{{\"type\":\"text\",\"text\":\"Synthetic sidechain output.\"}}]}}}}\n"
    )
}

#[test]
fn a_discovered_child_repeating_a_sidechains_uuid_degrades_instead_of_double_counting() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let parent = directory.path().join("parent.jsonl");
    let child = directory.path().join("child.jsonl");
    // The parent's main turn, then an inline sidechain rooted at
    // "shared-uuid".
    std::fs::write(
        &parent,
        claude_record("dup-parent", 1_760_000_000)
            + &claude_thread_record("shared-uuid", None, true, 1_760_000_001, "model-a"),
    )
    .expect("write parent");
    // The discovered child file repeats "shared-uuid" — child files are
    // authoritative, so the parent's own copy must not double count.
    std::fs::write(
        &child,
        claude_thread_record("shared-uuid", None, true, 1_760_000_002, "model-b"),
    )
    .expect("write child");

    let pass = evidence_pass_with_turn_rows(
        &[
            file_input(&parent, "dup-parent"),
            file_input(&child, "dup-child"),
        ],
        &|| false,
        Some(turn_row_store("claude", "dup-parent")),
    );
    let evidence = pass.evidence.expect("published evidence");

    assert_eq!(evidence.diagnostics.duplicate_turn_identities, 1);
    assert!(matches!(
        evidence.models,
        EvidenceValue::Partial {
            reason: CoverageReason::AttributionIncomplete,
            ..
        }
    ));
}

#[test]
fn a_changed_source_publishes_neither_projection() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let path = directory.path().join("changed.jsonl");
    std::fs::write(&path, claude_record("before", 1_760_000_000)).expect("write source");
    let change_source = |_: usize, path: &std::path::Path| {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .expect("open source")
            .write_all(claude_record("after", 1_760_000_001).as_bytes())
            .expect("change source");
    };

    let pass = evidence_pass_with_hook(
        &[file_input(&path, "changed")],
        &|| false,
        &change_source,
        None,
    );

    assert_eq!(pass.outcome, PassOutcome::SourceChanged);
    assert!(pass.analysis.metrics.is_none());
    assert!(pass.evidence.is_none());
}

#[cfg(unix)]
#[test]
fn a_deleted_source_is_missing_not_unreadable() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::TempDir::new().expect("tempdir");
    let deleted = directory.path().join("deleted.jsonl");
    let unreadable = directory.path().join("unreadable.jsonl");
    std::fs::write(&deleted, claude_record("deleted", 1_760_000_000))
        .expect("write deleted source");
    std::fs::write(&unreadable, claude_record("unreadable", 1_760_000_000))
        .expect("write unreadable source");
    let deleted_input = file_input(&deleted, "deleted");
    let unreadable_input = file_input(&unreadable, "unreadable");
    std::fs::remove_file(&deleted).expect("remove source");
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000))
        .expect("remove read permission");

    let missing = evidence_pass(&[deleted_input], &|| false);
    let unreadable_pass = evidence_pass(&[unreadable_input], &|| false);
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o600))
        .expect("restore read permission");

    assert_eq!(missing.outcome, PassOutcome::SourceMissing);
    assert_eq!(
        unreadable_pass.outcome,
        PassOutcome::Unreadable(UnreadableReason::ClaimFailed)
    );
}

#[test]
fn an_unsupported_schema_terminates() {
    let pass = evidence_pass(
        &[SessionInput {
            agent: "claude".to_string(),
            session_id: "unsupported".to_string(),
            source: RawSource::Sqlite(std::path::PathBuf::from("unsupported.sqlite")),
        }],
        &|| false,
    );

    assert_eq!(pass.outcome, PassOutcome::Unsupported);
    assert!(pass.analysis.metrics.is_none());
    assert!(pass.evidence.is_none());
}

#[test]
fn a_pass_signal_counts_every_record_and_carries_cancellation() {
    use std::io::Cursor;

    let first = PassSignal::new();
    let second = PassSignal::new();
    let mut reader =
        antiburn_local::analysis::BoundedJsonlReader::new(Cursor::new(b"one\ntwo\n".as_slice()));
    while reader.next_record(&|| first.observe()).is_some() {}

    assert!(first.progress() > 0);
    assert!(!first.observe());
    first.cancel();
    assert!(first.observe());
    assert!(!second.observe());
    assert_eq!(second.progress(), 1);
}

#[test]
fn an_inline_source_records_matching_and_mismatching_generations() {
    let content = claude_record("inline", 1_760_000_000);
    let fingerprint = inline_fingerprint(&content);
    let matching = ClaimedSource {
        fingerprint: Some(fingerprint),
        generation: 9,
    };
    let mismatching = ClaimedSource {
        fingerprint: Some("sv1:different".to_string()),
        generation: 9,
    };
    let actual = inline_fingerprint(&content);
    let StreamOutcome::Published { session, .. } = stream_vendor(
        &[SessionInput {
            agent: "claude".to_string(),
            session_id: "inline".to_string(),
            source: RawSource::Jsonl(content),
        }],
        &CancelFlag::never(),
    ) else {
        panic!("inline source must publish");
    };
    let key = SessionKey::new("native", "claude-code", "inline");
    let matching_analysis = SessionAnalysis {
        metrics: Some(session.parent.clone()),
        analyzed_generation: attributed_generation(&matching, Some(&actual)),
        ..SessionAnalysis::unavailable()
    };
    let mismatching_analysis = SessionAnalysis {
        metrics: Some(session.parent),
        analyzed_generation: attributed_generation(&mismatching, Some(&actual)),
        ..SessionAnalysis::unavailable()
    };

    assert_eq!(
        matching_analysis
            .record(&key)
            .expect("matching analysis record")
            .analyzed_generation,
        9
    );
    assert_eq!(
        mismatching_analysis
            .record(&key)
            .expect("mismatching analysis record")
            .analyzed_generation,
        0
    );
}

#[test]
fn cancellation_during_a_child_read_publishes_and_persists_nothing() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let parent = directory.path().join("parent.jsonl");
    let child = directory.path().join("child.jsonl");
    std::fs::write(&parent, claude_record("parent", 1_760_000_000)).expect("write parent");
    let child_content = format!(
        "{}{}",
        claude_record("child-first", 1_760_000_001),
        claude_record("child-second", 1_760_000_002)
    );
    std::fs::write(&child, child_content).expect("write child");
    let reading_child = std::cell::Cell::new(false);
    let child_cancel_checks = std::cell::Cell::new(0);
    let cancelled = || {
        if !reading_child.get() {
            return false;
        }
        let checks = child_cancel_checks.get();
        if checks >= 3 {
            return true;
        }
        child_cancel_checks.set(checks + 1);
        checks + 1 >= 3
    };
    let hook = |index: usize, _: &std::path::Path| {
        if index == 1 {
            reading_child.set(true);
        }
    };

    let analysis = match stream_vendor_with_hooks(
        &[file_input(&parent, "parent"), file_input(&child, "child")],
        &cancelled,
        &hook,
        None,
        None,
    ) {
        StreamOutcome::ParentUnreadable(reason) => {
            assert_eq!(reason, UnreadableReason::Cancelled);
            SessionAnalysis::unavailable()
        }
        StreamOutcome::Published { .. } => panic!("cancelled child read must not publish"),
        StreamOutcome::SourceChanged => panic!("cancelled child read is not a source change"),
        StreamOutcome::ParentMissing | StreamOutcome::ParentUnsupported => {
            panic!("cancelled child read must stay unreadable")
        }
    };

    assert_eq!(child_cancel_checks.get(), 3);
    assert!(analysis.metrics.is_none());
    assert!(analysis.summary.is_none());
    assert!(
        analysis
            .record(&SessionKey::new("native", "claude-code", "parent"))
            .is_none()
    );
}

#[test]
fn a_cancelled_pass_publishes_nothing() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let path = directory.path().join("parent.jsonl");
    std::fs::write(&path, claude_record("parent", 1_760_000_000)).expect("write parent");
    let flag = CancelFlag(Arc::new(AtomicBool::new(true)));

    assert!(matches!(
        stream_vendor(&[file_input(&path, "parent")], &flag),
        StreamOutcome::ParentUnreadable(UnreadableReason::Cancelled)
    ));
}

#[test]
fn a_session_written_moments_ago_reads_as_active() {
    let now = 1_800_000_000;
    assert!(is_active(Some(now - 5), now));
    assert!(is_active(Some(now), now));
    assert!(!is_active(Some(now - ACTIVE_SESSION_WINDOW_SECS - 1), now));
    assert!(!is_active(None, now));
}

#[test]
fn inclusive_model_runs_put_parent_modes_before_subagent_modes() {
    let parent = vec![
        ModelRun {
            model: "claude-opus-4-6".to_string(),
            thinking_mode: Some("high".to_string()),
        },
        ModelRun {
            model: "gpt-5.6-sol".to_string(),
            thinking_mode: Some("xhigh".to_string()),
        },
    ];
    let child = vec![
        ModelRun {
            model: "claude-fable-5".to_string(),
            thinking_mode: Some("high".to_string()),
        },
        ModelRun {
            model: "claude-haiku-4-5".to_string(),
            thinking_mode: Some("low".to_string()),
        },
        ModelRun {
            model: "gpt-5.6-sol".to_string(),
            thinking_mode: Some("xhigh".to_string()),
        },
    ];

    assert_eq!(
        model_runs_parent_first_lists(parent.clone(), [child.clone()].into_iter()),
        vec![
            parent[0].clone(),
            parent[1].clone(),
            child[0].clone(),
            child[1].clone(),
        ]
    );
}

#[test]
fn model_runs_are_trimmed_without_losing_the_thinking_mode() {
    assert_eq!(
        normalize_model_run(&ModelRun {
            model: " gpt-5.6-sol ".to_string(),
            thinking_mode: Some(" xhigh ".to_string()),
        }),
        Some(ModelRun {
            model: "gpt-5.6-sol".to_string(),
            thinking_mode: Some("xhigh".to_string()),
        })
    );
}

#[test]
fn cached_inclusive_model_runs_reject_invalid_json_and_normalize_values() {
    assert!(cached_inclusive_model_runs("not json").is_empty());
    assert_eq!(
        cached_inclusive_model_runs(
            r#"[{"model":" model-b ","thinkingMode":" high "},{"model":"model-b","thinkingMode":"high"},{"model":""}]"#,
        ),
        vec![ModelRun {
            model: "model-b".to_string(),
            thinking_mode: Some("high".to_string()),
        }]
    );
}

#[test]
fn a_missing_transcript_fingerprints_as_missing() {
    let source = SessionSource::File("/does/not/exist/session.jsonl".into());
    assert_eq!(fingerprint_of(&source), MISSING_FINGERPRINT);
    assert_eq!(
        source_path(&source).as_deref(),
        Some("/does/not/exist/session.jsonl")
    );

    let inline = SessionSource::Inline {
        label: "opencode:abc".into(),
        content: "{}".into(),
    };
    assert_eq!(fingerprint_of(&inline), MISSING_FINGERPRINT);
    assert_eq!(source_path(&inline), None);
}

#[test]
fn a_child_transcript_change_updates_the_combined_fingerprint() {
    let directory = tempfile::TempDir::new().unwrap();
    let parent = directory.path().join("parent.jsonl");
    let child = directory.path().join("child.jsonl");
    std::fs::write(&parent, "parent").unwrap();
    std::fs::write(&child, "child").unwrap();
    let source = SessionSource::File(parent);

    let before = combined_fingerprint(&source, std::slice::from_ref(&child));
    assert!(before.starts_with(&format!("v{ANALYSIS_FINGERPRINT_VERSION}:")));
    std::fs::write(&child, "child has more model events").unwrap();

    assert_ne!(
        before,
        combined_fingerprint(&source, std::slice::from_ref(&child))
    );
}

#[test]
fn cached_costs_re_price_from_the_stored_breakdown() {
    // `ModelTokens` serializes snake_case (it carries no `rename_all`), and
    // the cache is written and read with that same type, so the stored
    // spelling is snake_case by construction.
    let stored = serde_json::to_string(&HashMap::from([(
        "claude-opus-4-6".to_string(),
        ModelTokens {
            input_tokens: 1_000_000,
            ..ModelTokens::default()
        },
    )]))
    .unwrap();

    let (cost, models) = price_cached_breakdown(&stored);
    assert_eq!(models, vec!["claude-opus-4-6".to_string()]);
    let cost = cost.expect("a known model prices");
    assert!((cost.input_usd - 5.0).abs() < 1e-9);

    // An unpriceable model yields no estimate rather than a wrong zero.
    let unknown = serde_json::to_string(&HashMap::from([(
        "some-unreleased-model".to_string(),
        ModelTokens::default(),
    )]))
    .unwrap();
    let (cost, models) = price_cached_breakdown(&unknown);
    assert!(cost.is_none());
    assert_eq!(models, vec!["some-unreleased-model".to_string()]);

    // Garbage in the cache degrades to "unknown", never to a panic.
    assert_eq!(price_cached_breakdown("not json").0, None);
}

fn tokens(input: u64) -> ModelTokens {
    ModelTokens {
        input_tokens: input,
        ..ModelTokens::default()
    }
}

#[test]
fn merging_breakdowns_sums_a_model_used_by_the_parent_and_a_sub_agent() {
    let parent = HashMap::from([("claude-opus-4-6".to_string(), tokens(100))]);
    let child = HashMap::from([("claude-opus-4-6".to_string(), tokens(50))]);

    let merged = merge_model_breakdowns([&parent, &child]);

    assert_eq!(merged.len(), 1);
    assert_eq!(merged["claude-opus-4-6"].input_tokens, 150);
}

#[test]
fn merging_breakdowns_keeps_a_model_only_one_side_used() {
    let parent = HashMap::from([("claude-opus-4-6".to_string(), tokens(100))]);
    let child_a = HashMap::from([
        ("claude-opus-4-6".to_string(), tokens(50)),
        ("claude-sonnet-4-5".to_string(), tokens(20)),
    ]);
    let child_b = HashMap::from([("gpt-5.6".to_string(), tokens(10))]);

    let merged = merge_model_breakdowns([&parent, &child_a, &child_b]);

    assert_eq!(merged.len(), 3);
    assert_eq!(merged["claude-opus-4-6"].input_tokens, 150);
    assert_eq!(merged["claude-sonnet-4-5"].input_tokens, 20);
    assert_eq!(merged["gpt-5.6"].input_tokens, 10);
}

#[test]
fn merging_no_breakdowns_yields_an_empty_map() {
    assert!(merge_model_breakdowns(std::iter::empty()).is_empty());
}

#[test]
fn an_inclusive_breakdown_prices_the_parent_and_every_sub_agent_together() {
    // This test mirrors the bug this rollup fixes. A parent spends
    // little. Its sub-agents together spend much more. The session's
    // cost must not show only the parent's price.
    let parent = HashMap::from([("claude-opus-4-6".to_string(), tokens(1_000_000))]);
    let subagent_a = HashMap::from([("claude-opus-4-6".to_string(), tokens(2_000_000))]);
    let subagent_b = HashMap::from([("claude-opus-4-6".to_string(), tokens(3_000_000))]);

    let top_level_cost = price_breakdown(&parent).expect("the parent alone prices");
    let inclusive = merge_model_breakdowns([&parent, &subagent_a, &subagent_b]);
    let inclusive_cost = price_breakdown(&inclusive).expect("the merged breakdown prices");

    // 1M + 2M + 3M input tokens of the same model total 6x the parent alone.
    assert!((inclusive_cost.total_usd - top_level_cost.total_usd * 6.0).abs() < 1e-6);
    assert!(inclusive_cost.total_usd > top_level_cost.total_usd);
}

/// A full `ModelTokens`, so the token-sum test below exercises every
/// billable component, not only input tokens.
fn full_tokens(input: u64, output: u64, cache_read: u64, cache_creation: u64) -> ModelTokens {
    ModelTokens {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_creation,
        cache_creation_1h_tokens: 0,
    }
}

#[test]
fn the_inclusive_token_sum_equals_the_parent_sum_plus_the_sub_agents_sum() {
    let parent = HashMap::from([("claude-opus-4-6".to_string(), full_tokens(100, 20, 5, 3))]);
    let subagent_a = HashMap::from([("claude-opus-4-6".to_string(), full_tokens(50, 10, 2, 1))]);
    let subagent_b = HashMap::from([("claude-sonnet-4-5".to_string(), full_tokens(30, 6, 1, 0))]);

    let subagents_merged = merge_model_breakdowns([&subagent_a, &subagent_b]);
    let inclusive_merged = merge_model_breakdowns([&parent, &subagent_a, &subagent_b]);

    let parent_sum = sum_billable_tokens(&parent);
    let subagents_sum = sum_billable_tokens(&subagents_merged);
    let inclusive_sum = sum_billable_tokens(&inclusive_merged);

    assert_eq!(
        inclusive_sum,
        BillableTokens {
            input_tokens: parent_sum.input_tokens + subagents_sum.input_tokens,
            output_tokens: parent_sum.output_tokens + subagents_sum.output_tokens,
            cache_read_tokens: parent_sum.cache_read_tokens + subagents_sum.cache_read_tokens,
            cache_creation_tokens: parent_sum.cache_creation_tokens
                + subagents_sum.cache_creation_tokens,
        }
    );
}

#[test]
fn models_are_sorted_and_blank_keys_dropped() {
    let breakdown = HashMap::from([
        ("gpt-5.6".to_string(), ModelTokens::default()),
        ("claude-opus-4-6".to_string(), ModelTokens::default()),
        ("  ".to_string(), ModelTokens::default()),
    ]);
    assert_eq!(
        sorted_models(&breakdown),
        vec!["claude-opus-4-6".to_string(), "gpt-5.6".to_string()]
    );
}

#[test]
fn a_fork_observation_is_recovered_from_a_synthetic_header() {
    let header = serde_json::json!({
        "type": "session_meta",
        "metadata": {
            FORK_OBSERVATION_KEY: {
                "parent_agent": "cursor",
                "parent_agent_session_id": "parent-42",
                "fork_kind": "fork",
                "provider_fork_point_id": serde_json::Value::Null,
                "detection_source": "stable_id_prefix",
                "confidence": 100,
                "inherited_item_count": 12,
                "extractor_version": "1",
            }
        }
    });
    assert_eq!(
        find_fork_parent(&header, FORK_OBSERVATION_DEPTH).as_deref(),
        Some("parent-42")
    );
}

#[test]
fn a_codex_session_header_declares_its_fork_parent() {
    let header = serde_json::json!({
        "timestamp": "2026-08-22T04:05:01.756Z",
        "type": "session_meta",
        "payload": {
            "id": "child-42",
            "forked_from_id": "parent-42",
            "source": "cli",
            "thread_source": "user",
        }
    });
    assert_eq!(
        find_fork_parent(&header, FORK_OBSERVATION_DEPTH).as_deref(),
        Some("parent-42")
    );
}

#[test]
fn a_codex_fork_parent_requires_a_session_header_and_a_nonempty_id() {
    let message = serde_json::json!({
        "type": "response_item",
        "payload": { "forked_from_id": "not-a-parent" }
    });
    let empty = serde_json::json!({
        "type": "session_meta",
        "payload": { "forked_from_id": "  " }
    });
    assert_eq!(find_fork_parent(&message, FORK_OBSERVATION_DEPTH), None);
    assert_eq!(find_fork_parent(&empty, FORK_OBSERVATION_DEPTH), None);
}

#[test]
fn a_header_without_an_observation_yields_no_parent() {
    let header = serde_json::json!({ "type": "session_meta", "metadata": { "cwd": "/x" } });
    assert_eq!(find_fork_parent(&header, FORK_OBSERVATION_DEPTH), None);
    // A malformed observation is ignored rather than half-read.
    let broken = serde_json::json!({ FORK_OBSERVATION_KEY: { "parent_agent": "cursor" } });
    assert_eq!(find_fork_parent(&broken, FORK_OBSERVATION_DEPTH), None);
}

#[test]
fn the_observation_search_stops_at_its_depth_budget() {
    let deep = serde_json::json!({ "a": { "b": { "c": { "d": {
        FORK_OBSERVATION_KEY: { "parent_agent_session_id": "too-deep" }
    }}}}});
    assert_eq!(find_fork_parent(&deep, FORK_OBSERVATION_DEPTH), None);
}

#[test]
fn an_unavailable_analysis_caches_nothing() {
    let analysis = SessionAnalysis::unavailable();
    assert!(
        analysis
            .record(&SessionKey::new("native", "claude-code", "abc"))
            .is_none()
    );
}
