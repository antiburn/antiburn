use std::path::PathBuf;

use rusqlite::{Connection, params};
use serde_json::{Value, json};

use super::*;
use crate::store::SessionRecord;

pub(crate) struct SyntheticJournal {
    _directory: tempfile::TempDir,
    pub path: PathBuf,
    pub connection: Connection,
    pub sequence: i64,
    session_id: String,
}

impl SyntheticJournal {
    pub(crate) fn new(request_bytes: usize) -> Self {
        Self::named(request_bytes, "native-session")
    }

    pub(crate) fn named(request_bytes: usize, session_id: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(r#"PRAGMA journal_mode = WAL;
            CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, time_created INTEGER, time_updated INTEGER);
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
            CREATE TABLE event_sequence (aggregate_id TEXT PRIMARY KEY, seq INTEGER NOT NULL, owner_id TEXT);
            CREATE TABLE event (id TEXT PRIMARY KEY, aggregate_id TEXT NOT NULL, seq INTEGER NOT NULL, type TEXT NOT NULL, data TEXT NOT NULL);
             CREATE UNIQUE INDEX event_aggregate_seq_idx ON event (aggregate_id, seq);"#).unwrap();
        connection
            .execute(
                "INSERT INTO session VALUES (?1, NULL, 1000, 1000)",
                [session_id],
            )
            .unwrap();
        connection.execute("INSERT INTO message VALUES ('assistant', ?1, 1000, 1000,
            '{\"role\":\"assistant\",\"modelID\":\"synthetic\",\"tokens\":{\"input\":1,\"output\":1}}')", [session_id]).unwrap();
        let mut fixture = Self {
            _directory: directory,
            path,
            connection,
            sequence: -1,
            session_id: session_id.into(),
        };
        fixture.event(
            "session.created.1",
            json!({"sessionID":"native-session","info":{"id":"native-session"}}),
        );
        fixture.event(
            "message.updated.1",
            json!({"sessionID":"native-session","info":{"id":"request","role":"user"}}),
        );
        fixture.event("message.part.updated.1", json!({"sessionID":"native-session","part":{"id":"request-text","messageID":"request","type":"text","text":"x".repeat(request_bytes)}}));
        fixture.event("message.updated.1", json!({"sessionID":"native-session","info":{"id":"assistant","role":"assistant","parentID":"request"}}));
        fixture
    }

    pub(crate) fn event(&mut self, kind: &str, mut value: Value) {
        value["sessionID"] = json!(self.session_id);
        if kind == "session.created.1" || kind == "session.updated.1" {
            value["info"]["id"] = json!(self.session_id);
        }
        if value["part"].get("sessionID").is_some() {
            value["part"]["sessionID"] = json!(self.session_id);
        }
        self.sequence += 1;
        self.connection
            .execute(
                "INSERT INTO event VALUES (?1, ?5, ?2, ?3, ?4)",
                params![
                    format!("event-{}", self.sequence),
                    self.sequence,
                    kind,
                    value.to_string(),
                    self.session_id,
                ],
            )
            .unwrap();
        self.connection
            .execute(
                "INSERT INTO event_sequence VALUES (?2, ?1, 'owner')
            ON CONFLICT(aggregate_id) DO UPDATE SET seq = excluded.seq",
                params![self.sequence, self.session_id],
            )
            .unwrap();
    }

    pub(crate) fn result(&mut self, index: usize) {
        self.event("message.part.updated.1", json!({"sessionID":"native-session","part":{
            "id":format!("part-{index}"),"messageID":"assistant","sessionID":"native-session",
            "type":"tool","callID":format!("call-{index}"),"tool":"bash",
            "state":{"status":"error","input":{"command":"cargo test parser"},"error":format!("Parser failure {index}")}
        }}));
    }

    pub(crate) fn start_result(&mut self, index: usize) {
        self.event(
            "message.part.updated.1",
            json!({"sessionID":"native-session","part":{
                "id":format!("part-{index}"),"messageID":"assistant","sessionID":"native-session",
                "type":"tool","callID":format!("call-{index}"),"tool":"bash",
                "state":{"status":"running","input":{"command":"cargo test parser"}}
            }}),
        );
    }

    pub(crate) fn results(&mut self, range: std::ops::Range<usize>) {
        self.connection.execute_batch("BEGIN").unwrap();
        for index in range {
            self.result(index);
        }
        self.connection.execute_batch("COMMIT").unwrap();
    }

    pub(crate) fn source(&self) -> SessionSource {
        SessionSource::ProviderDb {
            agent: antiburn_local::model::AgentKind::OpenCode,
            db_path: self.path.clone(),
            session_id: self.session_id.clone(),
            source_format: Some(antiburn_local::analysis::SourceFormat::OpenCodeSqliteV1),
        }
    }

    pub(crate) fn record(&self, cwd: &str) -> SessionRecord {
        SessionRecord {
            key: SessionKey::new("native", "opencode", &self.session_id),
            source_kind: "providerDb".into(),
            source_label: format!("opencode:{}", self.session_id),
            wsl_distro: None,
            title: Some("Synthetic journal".into()),
            title_source: Some("explicit".into()),
            cwd: Some(cwd.into()),
            surface: "cli".into(),
            updated_at_epoch: Some(time::OffsetDateTime::now_utc().unix_timestamp()),
            activity_cursor: format!("event:{}", self.sequence),
            activity_source: "event".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: Some(format!("journal:{}", self.sequence)),
        }
    }

    pub(crate) fn index(&self, store: &Store, cwd: &str) {
        store
            .upsert_sessions(&[self.record(cwd)], &crate::agents::evidence_cohort())
            .unwrap();
    }

    pub(crate) fn enable_live(store: &Store, cwd: &str) {
        let scope = antiburn_checks::domain::ActivationScope {
            environment: "native".into(),
            session_cwd: cwd.into(),
            run: antiburn_checks::domain::RunScope::Live,
        };
        store.lock().execute(
            "INSERT INTO burn_check_activation (check_id, activation_scope_key, definition_revision,
                enabled, generation, bootstrap_sequence, applied_sequence, updated_at_epoch)
             VALUES ('synthetic-journal', ?1, 'synthetic', 1, 1, 0, 0, 100)
             ON CONFLICT(check_id, activation_scope_key) DO UPDATE SET enabled = 1",
            [serde_json::to_string(&scope).unwrap()],
        ).unwrap();
    }
}

pub(crate) async fn run_page(store: &Store, source: SessionSource, now: i64) -> bool {
    process_next_with(store, &|| now, &|_| {}, &|| {}, &|_| {
        let source = source.clone();
        Box::pin(async move { Some(source) })
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn cold_journal_reads_require_an_enabled_exact_native_live_scope_and_an_active_main_session()
{
    let now = 10_000;
    for case in [
        "missing",
        "disabled",
        "other-project",
        "other-environment",
        "replay",
        "inactive",
        "unknown-activity",
        "fork",
        "subagent",
        "remote",
        "wsl",
        "unassigned",
        "empty-cwd",
        "active",
        "active-boundary",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        let source = SyntheticJournal::new(16);
        let mut record = source.record("/synthetic");
        record.updated_at_epoch = match case {
            "inactive" => Some(now - antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS - 1),
            "unknown-activity" => None,
            "active-boundary" => Some(now - antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS),
            _ => Some(now),
        };
        match case {
            "fork" => record.fork_parent_session_id = Some("parent".into()),
            "remote" => record.key.environment_key = "remote:synthetic".into(),
            "wsl" => record.wsl_distro = Some("Synthetic".into()),
            "unassigned" => record.cwd = None,
            "empty-cwd" => record.cwd = Some(String::new()),
            _ => {}
        }
        store
            .upsert_sessions(&[record], &crate::agents::evidence_cohort())
            .unwrap();
        if case != "missing" {
            SyntheticJournal::enable_live(&store, "/synthetic");
        }
        let update = match case {
            "disabled" => Some("UPDATE burn_check_activation SET enabled = 0"),
            "other-project" => Some(
                "UPDATE burn_check_activation SET activation_scope_key = json_set(activation_scope_key, '$.session_cwd', '/synthetic/child')",
            ),
            "other-environment" => Some(
                "UPDATE burn_check_activation SET activation_scope_key = json_set(activation_scope_key, '$.environment', 'remote:synthetic')",
            ),
            "replay" => Some(
                "UPDATE burn_check_activation SET activation_scope_key = json_set(activation_scope_key, '$.run', json('{\"Replay\":\"synthetic\"}'))",
            ),
            "subagent" => Some(
                "INSERT INTO session_relation VALUES ('native', 'opencode', 'parent', 'subagent', 'native-session', NULL)",
            ),
            _ => None,
        };
        if let Some(update) = update {
            store.lock().execute(update, []).unwrap();
        }
        let located = std::sync::atomic::AtomicUsize::new(0);
        let located_source = source.source();
        let worked = process_next_with(&store, &|| now, &|_| {}, &|| {}, &|_| {
            located.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let source = located_source.clone();
            Box::pin(async move { Some(source) })
        })
        .await
        .unwrap();
        let admitted = matches!(case, "active" | "active-boundary");
        assert_eq!(worked, admitted, "{case}");
        assert_eq!(
            located.load(std::sync::atomic::Ordering::SeqCst),
            usize::from(admitted),
            "{case}"
        );
        if !admitted {
            assert_eq!(
                store.next_opencode_journal_attempt(now).unwrap(),
                None,
                "{case}"
            );
            assert_eq!(
                store
                    .lock()
                    .query_row(
                        "SELECT COUNT(*) FROM opencode_journal_delivery",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                0,
                "{case}"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn stalled_source_location_times_out_before_the_claim_lease_expires() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let source = SyntheticJournal::new(16);
    source.index(&store, "/synthetic");

    assert!(
        process_next_with(&store, &|| 100, &|_| {}, &|| {}, &|_| {
            Box::pin(std::future::pending())
        })
        .await
        .unwrap()
    );

    let receipt: (i64, i64, Option<i64>, Option<i64>) = store
        .lock()
        .query_row(
            "SELECT retry_count, blocked, next_attempt_at_epoch, lease_expires_at_epoch
               FROM opencode_journal_delivery",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(receipt, (1, 0, Some(130), None));
    assert_eq!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .last()
            .unwrap()
            .gap_reason
            .as_deref(),
        Some("journal-unavailable")
    );
}

#[tokio::test]
async fn productive_journals_rotate_durably_and_drain_after_activity_expires_without_a_clock_tick()
{
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut first = SyntheticJournal::named(16, "first");
    let mut second = SyntheticJournal::named(16, "second");
    first.index(&store, "/synthetic");
    second.index(&store, "/synthetic");
    store.lock().execute("UPDATE session SET updated_at_epoch = CASE session_id WHEN 'first' THEN 10000 ELSE 9999 END", []).unwrap();
    for source in [&first, &second] {
        assert!(run_page(&store, source.source(), 10_000).await);
    }
    for source in [&mut first, &mut second] {
        source.connection.execute_batch("BEGIN").unwrap();
        for _ in 0..4096 {
            source.event(
                "session.updated.1",
                json!({"sessionID":"native-session","info":{"id":"native-session"}}),
            );
        }
        source.start_result(0);
        source.result(0);
        source.connection.execute_batch("COMMIT").unwrap();
    }
    let order = std::sync::Mutex::new(Vec::new());
    let first_source = first.source();
    let second_source = second.source();
    for turn in 0..4 {
        assert!(
            process_next_with(
                &store,
                &|| 10_001,
                &|key| {
                    order.lock().unwrap().push(key.session_id.clone());
                },
                &|| {},
                &|key| {
                    let source = if key.session_id == "first" {
                        first_source.clone()
                    } else {
                        second_source.clone()
                    };
                    Box::pin(async move { Some(source) })
                }
            )
            .await
            .unwrap()
        );
        if turn == 0 {
            assert_eq!(
                store.next_opencode_journal_attempt(10_001).unwrap(),
                Some(10_001)
            );
            store
                .lock()
                .execute("UPDATE session SET updated_at_epoch = 1", [])
                .unwrap();
            drop(store);
            store = Store::open(directory.path()).unwrap();
            assert_eq!(store.recover_opencode_journal_claims().unwrap(), 0);
        }
    }
    assert_eq!(
        *order.lock().unwrap(),
        ["first", "first", "second", "second"]
    );
    for source in [&first, &second] {
        let current = store
            .native_tool_results_current(&source.record("/synthetic").key)
            .unwrap();
        assert_eq!(current.len(), 1);
        assert!(current[0].result.finality);
    }
}

#[tokio::test]
async fn dormant_journal_resume_sweeps_every_thirty_seconds_without_discovery_or_restart_replay() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.index(&store, "/synthetic");
    store
        .lock()
        .execute("UPDATE session SET updated_at_epoch = 10000", [])
        .unwrap();
    assert!(run_page(&store, source.source(), 10_000).await);
    let generation = store
        .lock()
        .query_row("SELECT source_generation FROM session", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    store
        .lock()
        .execute("UPDATE session SET updated_at_epoch = 1", [])
        .unwrap();
    assert!(run_page(&store, source.source(), 10_001).await);
    assert_eq!(
        store.next_opencode_journal_attempt(10_001).unwrap(),
        Some(10_031)
    );
    assert!(!run_page(&store, source.source(), 10_030).await);
    source.start_result(0);
    source.result(0);
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.recover_opencode_journal_claims().unwrap(), 0);
    assert!(!run_page(&store, source.source(), 10_030).await);
    assert!(run_page(&store, source.source(), 10_031).await);
    let results = store
        .native_tool_results_current(&source.record("/synthetic").key)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].result.finality);
    assert_eq!(
        results[0].result.admission(),
        antiburn_local::analysis::ToolResultAdmission::Admitted
    );
    assert_eq!(
        store
            .lock()
            .query_row("SELECT source_generation FROM session", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        generation
    );
    assert_eq!(
        store.next_opencode_journal_attempt(10_031).unwrap(),
        Some(10_032)
    );
    assert!(run_page(&store, source.source(), 10_032).await);
    assert_eq!(
        store.next_opencode_journal_attempt(10_032).unwrap(),
        Some(10_062)
    );
}

#[tokio::test]
async fn admitted_cold_claim_recovers_after_restart_when_session_activity_expires() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    source.index(&store, "/synthetic");
    store
        .lock()
        .execute("UPDATE session SET updated_at_epoch = 10000", [])
        .unwrap();
    let interrupted = store
        .claim_next_opencode_journal(10_000, 60)
        .unwrap()
        .unwrap();
    assert!(interrupted.cursor().unwrap().is_none());
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.recover_opencode_journal_claims().unwrap(), 1);
    assert_eq!(
        store.next_opencode_journal_attempt(20_000).unwrap(),
        Some(20_000)
    );
    assert!(run_page(&store, source.source(), 20_000).await);
    assert!(
        !store
            .renew_opencode_journal_lease(&interrupted, 20_000, 60)
            .unwrap()
    );
    assert_eq!(store.recover_opencode_journal_claims().unwrap(), 0);
    let results = store
        .native_tool_results_current(&source.record("/synthetic").key)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].result.finality);
}

#[tokio::test]
async fn empty_pages_advance_and_schedule_continuation_without_idle_spin() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut fixture = SyntheticJournal::new(16);
    fixture.index(&store, "/synthetic");
    assert!(run_page(&store, fixture.source(), 99).await);
    fixture.connection.execute_batch("BEGIN").unwrap();
    for _ in 4..4100 {
        fixture.event(
            "session.updated.1",
            json!({"sessionID":"native-session","info":{"id":"native-session"}}),
        );
    }
    fixture.start_result(0);
    fixture.result(0);
    fixture.connection.execute_batch("COMMIT").unwrap();
    fixture.index(&store, "/synthetic");
    assert!(run_page(&store, fixture.source(), 100).await);
    assert_eq!(store.next_opencode_journal_attempt(100).unwrap(), Some(100));
    assert!(run_page(&store, fixture.source(), 100).await);
    assert_eq!(store.next_opencode_journal_attempt(100).unwrap(), Some(101));
    assert!(!run_page(&store, fixture.source(), 100).await);
    let publications = store.native_tool_result_publications_after(0, 100).unwrap();
    assert_eq!(
        publications
            .iter()
            .filter(|event| event.payload.as_ref().is_some_and(|result| result.finality))
            .count(),
        1
    );
    assert_eq!(publications.last().unwrap().live_eligible, Some(true));
    assert!(publications.iter().all(|event| event.outcome
        != crate::store::native_tool_results::ToolResultPublicationOutcome::CoverageGap));
}

#[tokio::test]
async fn projection_publication_does_not_publish_a_native_journal_tail() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut fixture = SyntheticJournal::new(16);
    fixture.result(0);
    let agent = antiburn_local::model::AgentKind::OpenCode;
    let (latest, rows) = antiburn_local::discovery::Explorers::DISK
        .provider_db_fingerprint(&agent, &fixture.path, &fixture.session_id)
        .await
        .unwrap();
    let mut record = fixture.record("/synthetic");
    record.source_fingerprint = Some(format!("sv1:db:{latest}:{rows}"));
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    let claim = store
        .claim_next_evidence(&["opencode"], 100, LEASE_SECS)
        .unwrap()
        .unwrap();
    let located = fixture.source();
    let analyzer = |agent, session: String, _wsl, claimed, signal, rows, parent| {
        let source = located.clone();
        Box::pin(async move {
            analysis::analyze_located_for_evidence(
                agent,
                &session,
                claimed,
                signal,
                rows,
                parent,
                analysis::LocatedTranscripts {
                    source,
                    children: Vec::new(),
                },
            )
            .await
        }) as crate::insights_worker::PassFuture
    };
    let mut pass = crate::insights_worker::run_record_pass_with(
        &record,
        PassSignal::new(),
        claim.claim_fence,
        store.clone(),
        &analyzer,
    )
    .await;
    assert_eq!(pass.outcome, analysis::PassOutcome::Published);
    assert!(pass.tool_result_observations.is_empty());
    assert!(!pass.tool_result_observations_complete);
    pass.analysis.analyzed_generation = claim.source_generation;
    assert!(
        crate::insights_worker::apply_outcome(&store, &claim, &pass, 100)
            .unwrap()
            .applied
    );
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .is_empty()
    );
    assert!(run_page(&store, fixture.source(), 101).await);
    let publications = store.native_tool_result_publications_after(0, 10).unwrap();
    assert_eq!(publications.len(), 1);
    assert!(publications[0].payload.is_some());
    assert_eq!(publications[0].live_eligible, Some(true));
    fixture.start_result(1);
    fixture.result(1);
    fixture.index(&store, "/synthetic");
    assert!(run_page(&store, fixture.source(), 102).await);
    let publications = store.native_tool_result_publications_after(0, 10).unwrap();
    assert_eq!(publications.last().unwrap().live_eligible, Some(true));
}

#[tokio::test]
async fn worker_persists_prefix_changes_as_blocked_gaps_without_replaying_history() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    source.index(&store, "/synthetic");
    assert!(run_page(&store, source.source(), 100).await);
    source.result(1);
    source.index(&store, "/synthetic");
    source.connection.execute("UPDATE event SET data = replace(data, 'Parser failure 0', 'Changed failure') WHERE seq = 4", []).unwrap();
    assert!(run_page(&store, source.source(), 101).await);
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT last_error FROM opencode_journal_delivery",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "journal-prefix-changed"
    );
    assert!(!run_page(&store, source.source(), 1_000).await);
    let events = store.native_tool_result_publications_after(0, 10).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.payload.is_some())
            .count(),
        1
    );
    assert_eq!(
        events.last().unwrap().gap_reason.as_deref(),
        Some("journal-prefix-changed")
    );
}

#[tokio::test]
async fn a_reserved_journal_sequence_does_not_stall_or_replay_the_materialized_prefix() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    source.index(&store, "/synthetic");
    assert!(run_page(&store, source.source(), 100).await);
    source
        .connection
        .execute("UPDATE event_sequence SET seq = 100000", [])
        .unwrap();
    let mut record = source.record("/synthetic");
    record.source_fingerprint = Some("unsettled".into());
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(run_page(&store, source.source(), 101).await);
    assert!(run_page(&store, source.source(), 102).await);
    assert!(!run_page(&store, source.source(), 102).await);
    assert!(
        store
            .lock()
            .query_row(
                "SELECT last_error IS NULL FROM opencode_journal_delivery",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
    source.start_result(1);
    source.result(1);
    source.index(&store, "/synthetic");
    assert!(run_page(&store, source.source(), 103).await);
    let events = store.native_tool_result_publications_after(0, 10).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.payload.as_ref().is_some_and(|result| result.finality))
            .count(),
        2
    );
    assert_eq!(events.last().unwrap().live_eligible, Some(true));
    assert!(events.iter().all(|event| event.outcome
        != crate::store::native_tool_results::ToolResultPublicationOutcome::CoverageGap));
    assert!(
        store
            .lock()
            .query_row(
                "SELECT last_error IS NULL FROM opencode_journal_delivery",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
}

#[tokio::test]
async fn native_head_poll_delivers_new_calls_without_a_discovery_generation_change() {
    let store = Store::open_in_memory(std::path::Path::new("/tmp/journal-poll-test")).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.index(&store, "/synthetic");
    assert!(run_page(&store, source.source(), 100).await);
    source.start_result(0);
    source.result(0);
    assert!(!run_page(&store, source.source(), 100).await);
    assert!(run_page(&store, source.source(), 101).await);
    let published = store.native_tool_result_publications_after(0, 10).unwrap();
    assert_eq!(
        published
            .iter()
            .filter(|event| event.payload.as_ref().is_some_and(|result| result.finality))
            .count(),
        1
    );
    assert_eq!(published.last().unwrap().live_eligible, Some(true));
    assert!(run_page(&store, source.source(), 110).await);
    assert_eq!(
        store.native_tool_result_publications_after(0, 10).unwrap(),
        published
    );
    assert_eq!(store.next_opencode_journal_attempt(110).unwrap(), Some(111));

    let mut archive = source.record("/synthetic");
    archive.key.session_id = "archived-session".into();
    archive.source_label = "opencode:archived-session".into();
    archive.updated_at_epoch =
        Some(115 - antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS - 1);
    store
        .upsert_sessions(&[archive], &crate::agents::evidence_cohort())
        .unwrap();
    let claim = store.claim_next_opencode_journal(115, 60).unwrap().unwrap();
    assert_eq!(claim.key.session_id, "native-session");
    assert_eq!(store.recover_opencode_journal_claims().unwrap(), 1);
    let replacement = store.claim_next_opencode_journal(115, 60).unwrap().unwrap();
    assert!(!store.finish_opencode_journal_poll(&claim, 115).unwrap());
    assert!(
        store
            .finish_opencode_journal_poll(&replacement, 115)
            .unwrap()
    );
}

#[tokio::test]
async fn transient_failure_recovers_without_new_events_or_a_discovery_change() {
    use crate::store::native_tool_results::ToolResultPublicationOutcome;

    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let source = SyntheticJournal::new(16);
    source.index(&store, "/synthetic");
    assert!(run_page(&store, source.source(), 100).await);
    let cursor = store
        .lock()
        .query_row(
            "SELECT cursor_json FROM opencode_journal_delivery",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    let claim = store.claim_next_opencode_journal(105, 60).unwrap().unwrap();
    assert!(!claim.requires_full_read);
    assert!(
        store
            .fail_opencode_journal(&claim, JournalFailure::ReadFailed, 105, 110)
            .unwrap()
    );
    assert_eq!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .last()
            .unwrap()
            .outcome,
        ToolResultPublicationOutcome::CoverageGap
    );
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    let recovery = store.claim_next_opencode_journal(110, 60).unwrap().unwrap();
    assert!(recovery.requires_full_read);
    assert!(store.finish_opencode_journal_poll(&recovery, 110).is_err());
    store.recover_opencode_journal_claims().unwrap();
    assert!(run_page(&store, source.source(), 110).await);
    let recovered: (String, Option<String>, i64) = store
        .lock()
        .query_row(
            "SELECT cursor_json, last_error, retry_count FROM opencode_journal_delivery",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(recovered, (cursor, None, 0));
    assert_ne!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .last()
            .unwrap()
            .outcome,
        ToolResultPublicationOutcome::CoverageGap
    );
    assert_eq!(store.next_opencode_journal_attempt(110).unwrap(), Some(111));
}
