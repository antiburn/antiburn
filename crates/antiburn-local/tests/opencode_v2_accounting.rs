use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use antiburn_local::analysis::{
    CalculatedCost, EventSource, FenceScope, MemoryTurnRowStore, NormalizedRecord,
    NormalizedSession, RawSource, RecordSink, SessionInput, SessionMetricsAccumulator,
    SessionSummary, SourceAccounting, SourceFormat, TokenClasses, TurnRowSink, TurnScope,
    TurnSessionKey, UsageReconciliation, analyze_session, analyze_sources_with, merge_metrics,
    metrics_from_rows, metrics_from_rows_with_summaries, normalize_source, query_model_runs,
    query_turn_rows, reader_for_input,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

#[derive(Default)]
struct Capture {
    events: Vec<antiburn_local::analysis::NormalizedEvent>,
    summary: Option<SessionSummary>,
}

impl RecordSink for Capture {
    fn record(&mut self, record: NormalizedRecord) {
        if let NormalizedRecord::MetricsEvent(event) = record {
            self.events.push(*event);
        }
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.summary = Some(summary);
    }
}

fn fixture() -> (tempfile::TempDir, PathBuf, SessionInput) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native.db");
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE session_v2 (
           id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, title TEXT,
           time_created INTEGER, time_updated INTEGER, cost REAL,
           tokens_input INTEGER, tokens_output INTEGER, tokens_reasoning INTEGER,
           tokens_cache_read INTEGER, tokens_cache_write INTEGER);
         CREATE TABLE session_message (
           id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER,
           time_created INTEGER, time_updated INTEGER, data TEXT);
         CREATE UNIQUE INDEX native_seq ON session_message(session_id,seq);
         INSERT INTO session_v2 VALUES ('ses_accounting',NULL,'/repo','Accounting',1,2,
           .07,40,9,7,13,11);",
        )
        .unwrap();
    let input = SessionInput {
        agent: "opencode".into(),
        session_id: "ses_accounting".into(),
        source: RawSource::Sqlite(path.clone()),
        source_format: SourceFormat::OpenCodeSqliteV2,
        fork_parent_session_id: None,
    };
    (directory, path, input)
}

fn insert(path: &std::path::Path, id: &str, kind: &str, seq: i64, value: Value) {
    Connection::open(path)
        .unwrap()
        .execute(
            "INSERT INTO session_message VALUES (?1,'ses_accounting',?2,?3,1,2,?4)",
            params![id, kind, seq, value.to_string()],
        )
        .unwrap();
}

fn assistant() -> Value {
    json!({"time":{"created":1000},"agent":"build",
      "model":{"id":"claude-sonnet-4-6","providerID":"anthropic","variant":"native-high"},
      "tokens":{"input":10,"output":3,"reasoning":2,"cache":{"read":7,"write":5}},
      "cost":0.01,"content":[]})
}

struct SourceMetricsSink {
    parent_id: String,
    store: Arc<MemoryTurnRowStore>,
    current: Option<(SessionMetricsAccumulator, TurnRowSink)>,
    accumulators: BTreeMap<String, SessionMetricsAccumulator>,
    summaries: BTreeMap<String, SessionSummary>,
}

impl RecordSink for SourceMetricsSink {
    fn begin_source(&mut self, session_id: &str) {
        assert!(self.current.is_none());
        self.current = Some((
            SessionMetricsAccumulator::new("opencode", session_id),
            TurnRowSink::new(
                self.store.clone(),
                session_id,
                (session_id != self.parent_id).then_some(TurnScope::Delegated),
            ),
        ));
    }

    fn record(&mut self, mut record: NormalizedRecord) {
        let (metrics, rows) = self.current.as_mut().unwrap();
        rows.observe(&record);
        if let NormalizedRecord::MetricsEvent(event) = &mut record {
            event.source = EventSource::Parent;
        }
        metrics.record(record);
    }

    fn source_summary(&mut self, session_id: &str, summary: SessionSummary) {
        let (mut metrics, mut rows) = self.current.take().unwrap();
        rows.finish(summary.clone());
        assert!(!rows.has_error());
        metrics.finish(summary.clone());
        self.accumulators.insert(session_id.to_owned(), metrics);
        self.summaries.insert(session_id.to_owned(), summary);
    }

    fn finish(&mut self, _: SessionSummary) {
        assert!(self.current.is_none());
    }
}

fn opposite_source_fixture(inconsistent_parent: bool) -> (tempfile::TempDir, SessionInput) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!(
            "../src/discovery/agents/tests/fixtures/opencode_pinned_mixed.sql"
        ))
        .unwrap();
    for (session_id, parent_id, inconsistent) in [
        ("ses_parent", None, inconsistent_parent),
        ("ses_child", Some("ses_parent"), !inconsistent_parent),
    ] {
        let aggregate = u64::from(!inconsistent);
        let created = if parent_id.is_some() { 2000 } else { 1000 };
        connection
            .execute(
                "INSERT INTO session_v2 (id,project_id,parent_id,slug,directory,version,
             time_created,time_updated,cost,tokens_reasoning)
             VALUES (?1,'global',?2,?1,'/repo','2.0.32',?3,?3,?4,?4)",
                params![session_id, parent_id, created, aggregate],
            )
            .unwrap();
        let mut message = assistant();
        message["tokens"] = json!({
            "input":0,"output":0,"reasoning":u64::from(inconsistent),
            "cache":{"read":0,"write":0}
        });
        message["cost"] = json!(u64::from(inconsistent));
        message["time"]["created"] = json!(created);
        connection
            .execute(
                "INSERT INTO session_message VALUES (?1,?2,'assistant',0,?3,?3,?4)",
                params![
                    format!("msg_{session_id}"),
                    session_id,
                    created,
                    message.to_string()
                ],
            )
            .unwrap();
    }
    let input = SessionInput {
        agent: "opencode".into(),
        session_id: "ses_parent".into(),
        source: RawSource::Sqlite(path),
        source_format: SourceFormat::OpenCodeSqliteV2,
        fork_parent_session_id: None,
    };
    (directory, input)
}

#[test]
fn normalized_cluster_keeps_source_reconciliation_through_streaming_replay_and_json() {
    for inconsistent_parent in [true, false] {
        let (_directory, input) = opposite_source_fixture(inconsistent_parent);
        let normalized = normalize_source(&input).unwrap();
        let normalized: NormalizedSession =
            serde_json::from_value(serde_json::to_value(normalized).unwrap()).unwrap();
        let analyzed = analyze_session(&normalized);
        let accounting = analyzed.accounting.unwrap();
        assert_eq!(accounting.aggregate_tokens, accounting.retained_tokens);
        assert_eq!(
            accounting.aggregate_calculated_cost,
            accounting.retained_calculated_cost
        );
        assert_eq!(
            accounting.token_reconciliation(),
            UsageReconciliation::Inconsistent
        );
        assert_eq!(
            accounting.cost_reconciliation(),
            UsageReconciliation::Inconsistent
        );
        assert_eq!(
            analyzed.token_reconciliation,
            Some(UsageReconciliation::Inconsistent)
        );
        assert_eq!(
            analyzed.cost_reconciliation,
            Some(UsageReconciliation::Inconsistent)
        );

        let store = MemoryTurnRowStore::new("opencode", &input.session_id);
        let mut sink = SourceMetricsSink {
            parent_id: input.session_id.clone(),
            store: store.clone(),
            current: None,
            accumulators: BTreeMap::new(),
            summaries: BTreeMap::new(),
        };
        reader_for_input(&input).visit(&input, &mut sink).unwrap();
        assert_eq!(sink.accumulators.len(), 2);
        for (source_id, summary) in &sink.summaries {
            let accounting = summary.accounting.unwrap();
            if (source_id == &input.session_id) == inconsistent_parent {
                assert_eq!(
                    accounting.token_reconciliation(),
                    UsageReconciliation::Inconsistent
                );
                assert_eq!(
                    accounting.cost_reconciliation(),
                    UsageReconciliation::Inconsistent
                );
            } else {
                assert!(matches!(
                    accounting.token_reconciliation(),
                    UsageReconciliation::Unallocated(TokenClasses {
                        reasoning_tokens: 1,
                        ..
                    })
                ));
                assert_eq!(
                    accounting.cost_reconciliation(),
                    UsageReconciliation::Unallocated(
                        CalculatedCost::producer_calculated(1.0).unwrap()
                    )
                );
            }
        }
        let parent = sink.accumulators.remove(&input.session_id).unwrap();
        let children = sink.accumulators.into_values().collect::<Vec<_>>();
        let streaming = merge_metrics(&parent, &children);
        let rows = store.with_connection(|connection| {
            query_turn_rows(
                connection,
                &TurnSessionKey {
                    environment_key: "native",
                    agent: "opencode",
                    session_id: &input.session_id,
                },
                &FenceScope::single(1),
            )
            .unwrap()
        });
        assert_eq!(rows.len(), 2);
        let summaries =
            serde_json::from_value(serde_json::to_value(sink.summaries).unwrap()).unwrap();
        let replay =
            metrics_from_rows_with_summaries("opencode", &input.session_id, &rows, &summaries)
                .unwrap();
        let aggregate = analyze_sources_with(vec![input], false);
        assert_eq!(aggregate.sessions.len(), 1);
        let expected = serde_json::to_value(&analyzed).unwrap();
        assert_eq!(serde_json::to_value(streaming).unwrap(), expected);
        assert_eq!(serde_json::to_value(replay).unwrap(), expected);
        assert_eq!(
            serde_json::to_value(&aggregate.sessions[0]).unwrap(),
            expected
        );
        assert_eq!(expected["tokenReconciliation"]["status"], "inconsistent");
        assert_eq!(expected["costReconciliation"]["status"], "inconsistent");
    }
}

#[test]
fn source_reconciliation_fields_are_optional_for_existing_json() {
    let accounting = SourceAccounting::default();
    let json = serde_json::to_value(accounting).unwrap();
    assert!(json.get("sourceTokenReconciliation").is_none());
    assert!(json.get("sourceCostReconciliation").is_none());
    assert_eq!(
        serde_json::from_value::<SourceAccounting>(json).unwrap(),
        accounting
    );
    let input = SessionInput {
        agent: "opencode".into(),
        session_id: "ses_v1".into(),
        source: RawSource::Jsonl(String::new()),
        source_format: SourceFormat::OpenCodeJsonl,
        fork_parent_session_id: None,
    };
    let session = normalize_source(&input).unwrap();
    assert!(
        serde_json::to_value(&session)
            .unwrap()
            .get("accounting")
            .is_none()
    );
    let metrics = serde_json::to_value(analyze_session(&session)).unwrap();
    assert!(metrics.get("accounting").is_none());
    assert!(metrics.get("tokenReconciliation").is_none());
    assert!(metrics.get("costReconciliation").is_none());
}

#[test]
fn native_accounting_survives_rows_replay_and_keeps_priced_output_combined_once() {
    let (_directory, path, input) = fixture();
    insert(&path, "msg_step", "assistant", 0, assistant());
    insert(
        &path,
        "msg_compact",
        "compaction",
        1,
        json!({
            "time":{"created":2000},"status":"completed","reason":"auto",
            "model":{"id":"claude-haiku-4-5","providerID":"anthropic","variant":"compact"},
            "tokens":{"input":20,"output":4,"reasoning":1,"cache":{"read":3,"write":2}},
            "cost":0.02,"summary":"not a request","recent":"opaque"
        }),
    );
    let mut capture = Capture::default();
    reader_for_input(&input)
        .visit(&input, &mut capture)
        .unwrap();
    assert_eq!(capture.events.len(), 2);
    let step = capture.events[0].accounting.clone().unwrap();
    assert_eq!(step.native_variant.as_deref(), Some("native-high"));
    assert!(capture.events[0].thinking_mode.is_none());
    assert_eq!(step.token_classes.unwrap().reasoning_tokens, 2);
    assert_eq!(capture.events[0].usage.output_tokens, 5);
    let summary = capture.summary.unwrap();
    let accounting = summary.accounting.unwrap();
    let normalized = reader_for_input(&input).normalize(&input).unwrap();
    assert_eq!(normalized.accounting, Some(accounting));
    assert_eq!(
        antiburn_local::analysis::analyze_session(&normalized).accounting,
        Some(accounting)
    );
    assert_eq!(accounting.retained_tokens.unwrap().visible_output_tokens, 7);
    assert_eq!(accounting.retained_tokens.unwrap().reasoning_tokens, 3);
    assert_eq!(
        accounting.token_reconciliation(),
        UsageReconciliation::Unallocated(TokenClasses {
            input_tokens: 10,
            visible_output_tokens: 2,
            reasoning_tokens: 4,
            cache_read_tokens: 3,
            cache_write_tokens: 4,
        })
    );
    assert!((accounting.retained_calculated_cost.unwrap().usd - 0.03).abs() < 1e-12);
    assert!(
        matches!(accounting.cost_reconciliation(), UsageReconciliation::Unallocated(cost) if (cost.usd-0.04).abs() < 1e-12)
    );

    let store = MemoryTurnRowStore::new("opencode", "ses_accounting");
    let mut sink = TurnRowSink::new(store.clone(), "ses_accounting", None);
    let mut metrics = SessionMetricsAccumulator::new("opencode", "ses_accounting");
    for event in capture.events {
        sink.record(NormalizedRecord::MetricsEvent(Box::new(event.clone())));
        metrics.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    }
    sink.finish(summary.clone());
    metrics.finish(summary.clone());
    let key = TurnSessionKey {
        environment_key: "native",
        agent: "opencode",
        session_id: "ses_accounting",
    };
    let rows = store.with_connection(|connection| {
        query_turn_rows(connection, &key, &FenceScope::single(1)).unwrap()
    });
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].accounting.as_ref(), Some(&step));
    let replay =
        metrics_from_rows("opencode", "ses_accounting", &rows, |_| summary.clone()).unwrap();
    assert_eq!(replay.accounting, Some(accounting));
    assert_eq!(replay.billable_output_tokens, 10);
    assert_eq!(replay.cost, metrics.metrics().cost);
    assert_eq!(replay.model_runs, metrics.metrics().model_runs);
    let runs = store.with_connection(|connection| {
        query_model_runs(connection, &key, &FenceScope::single(1)).unwrap()
    });
    assert_eq!(runs.len(), 2);
    assert!(
        runs.iter()
            .any(|run| run.native_variant.as_deref() == Some("native-high"))
    );
    assert!(
        runs.iter()
            .any(|run| run.native_variant.as_deref() == Some("compact"))
    );
    assert!(runs.iter().all(|run| run.thinking_mode.is_none()));
}

#[test]
fn native_reconciliation_keeps_unknown_inconsistent_zero_and_overflow_distinct() {
    let unknown = SourceAccounting::default();
    assert_eq!(unknown.token_reconciliation(), UsageReconciliation::Unknown);
    assert_eq!(unknown.cost_reconciliation(), UsageReconciliation::Unknown);
    let zero = SourceAccounting {
        aggregate_tokens: Some(TokenClasses::default()),
        aggregate_calculated_cost: CalculatedCost::producer_calculated(0.0),
        ..unknown
    };
    assert_eq!(zero.token_reconciliation(), UsageReconciliation::Matched);
    assert_eq!(zero.cost_reconciliation(), UsageReconciliation::Matched);
    let inconsistent = SourceAccounting {
        retained_tokens: Some(TokenClasses {
            reasoning_tokens: 1,
            ..TokenClasses::default()
        }),
        retained_calculated_cost: CalculatedCost::producer_calculated(1.0),
        ..zero
    };
    assert_eq!(
        inconsistent.token_reconciliation(),
        UsageReconciliation::Inconsistent
    );
    assert_eq!(
        inconsistent.cost_reconciliation(),
        UsageReconciliation::Inconsistent
    );
    assert!(CalculatedCost::producer_calculated(f64::INFINITY).is_none());
    assert!(CalculatedCost::producer_calculated(-1.0).is_none());
    assert!(
        TokenClasses {
            visible_output_tokens: u64::MAX,
            ..TokenClasses::default()
        }
        .checked_add(TokenClasses {
            reasoning_tokens: 1,
            ..TokenClasses::default()
        })
        .is_none()
    );
    let maximum = TokenClasses {
        input_tokens: u64::MAX,
        ..TokenClasses::default()
    };
    assert!(
        maximum
            .checked_add(TokenClasses {
                input_tokens: 1,
                ..TokenClasses::default()
            })
            .is_none()
    );
    let overflow = SourceAccounting {
        retained_tokens: Some(maximum),
        ..unknown
    }
    .checked_merge(SourceAccounting {
        retained_tokens: Some(TokenClasses {
            input_tokens: 1,
            ..TokenClasses::default()
        }),
        ..unknown
    });
    assert!(overflow.retained_tokens.is_none());
    assert_eq!(
        inconsistent.token_reconciliation().checked_merge(
            UsageReconciliation::Unallocated(TokenClasses {
                reasoning_tokens: 1,
                ..TokenClasses::default()
            }),
            TokenClasses::checked_add,
        ),
        UsageReconciliation::Inconsistent,
    );
}

#[test]
fn malformed_accounting_keeps_valid_steps_and_does_not_create_remainder_events() {
    let (_directory, path, input) = fixture();
    let mut malformed = assistant();
    malformed["tokens"]["reasoning"] = json!(-1);
    malformed["cost"] = json!("not a cost");
    malformed["model"]["variant"] = json!(false);
    insert(&path, "msg_bad", "assistant", 0, malformed);
    insert(&path, "msg_valid", "assistant", 1, assistant());
    Connection::open(&path)
        .unwrap()
        .execute("UPDATE session_v2 SET cost='invalid'", [])
        .unwrap();
    let mut capture = Capture::default();
    reader_for_input(&input)
        .visit(&input, &mut capture)
        .unwrap();
    assert_eq!(capture.events.len(), 2);
    assert_eq!(capture.events[0].accounting, Some(Default::default()));
    assert_eq!(capture.events[1].usage.output_tokens, 5);
    let summary = capture.summary.unwrap();
    assert!(!summary.coverage_gaps.is_empty());
    assert_eq!(
        summary.accounting.unwrap().cost_reconciliation(),
        UsageReconciliation::Unknown
    );
}

#[test]
fn inclusive_accounting_does_not_cancel_inconsistent_source_totals() {
    let mut parent = SessionMetricsAccumulator::new("opencode", "parent");
    parent.finish(SessionSummary {
        accounting: Some(SourceAccounting {
            aggregate_tokens: Some(TokenClasses::default()),
            retained_tokens: Some(TokenClasses {
                reasoning_tokens: 1,
                ..TokenClasses::default()
            }),
            aggregate_calculated_cost: CalculatedCost::producer_calculated(0.0),
            retained_calculated_cost: CalculatedCost::producer_calculated(1.0),
            ..SourceAccounting::default()
        }),
        ..SessionSummary::default()
    });
    let mut child = SessionMetricsAccumulator::new("opencode", "child");
    child.finish(SessionSummary {
        accounting: Some(SourceAccounting {
            aggregate_tokens: Some(TokenClasses {
                reasoning_tokens: 1,
                ..TokenClasses::default()
            }),
            retained_tokens: Some(TokenClasses::default()),
            aggregate_calculated_cost: CalculatedCost::producer_calculated(1.0),
            retained_calculated_cost: CalculatedCost::producer_calculated(0.0),
            ..SourceAccounting::default()
        }),
        ..SessionSummary::default()
    });
    let merged = antiburn_local::analysis::merge_metrics(&parent, &[child]);
    assert_eq!(
        merged.token_reconciliation,
        Some(UsageReconciliation::Inconsistent)
    );
    assert_eq!(
        merged.cost_reconciliation,
        Some(UsageReconciliation::Inconsistent)
    );
    assert_eq!(merged.event_count, 0);
    assert_eq!(
        merged.accounting.unwrap().token_reconciliation(),
        UsageReconciliation::Inconsistent
    );
    assert_eq!(
        merged.accounting.unwrap().cost_reconciliation(),
        UsageReconciliation::Inconsistent
    );
}

#[test]
fn native_variant_history_keeps_the_metrics_memory_bound() {
    use antiburn_local::analysis::{
        NormalizedEvent, RETAINED_METRICS_BYTES_BOUND, Role, StepAccounting, Usage,
    };

    let mut metrics = SessionMetricsAccumulator::new("opencode", "bounded");
    let store = MemoryTurnRowStore::new("opencode", "bounded");
    let mut rows = TurnRowSink::new(store.clone(), "bounded", None);
    for index in 0..1000 {
        let mut event = NormalizedEvent::new(Role::Assistant);
        event.ts_ms = Some(i64::from(index) * 1000);
        event.model = Some("claude-sonnet-4-6".into());
        event.usage = Usage {
            output_tokens: 1,
            ..Usage::default()
        };
        event.accounting = Some(StepAccounting {
            native_variant: Some(format!(
                "{}-{index}",
                "v".repeat(antiburn_local::analysis::EVIDENCE_STRING_CAP - 10)
            )),
            ..StepAccounting::default()
        });
        rows.record(NormalizedRecord::MetricsEvent(Box::new(event.clone())));
        metrics.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    }
    metrics.finish(SessionSummary::default());
    rows.finish(SessionSummary::default());
    assert_eq!(metrics.metrics().model_runs.len(), 32);
    assert!(metrics.retained_bytes() <= RETAINED_METRICS_BYTES_BOUND);
    assert!(
        metrics
            .metrics()
            .model_runs
            .iter()
            .all(|run| run.thinking_mode.is_none())
    );
    let runs = store.with_connection(|connection| {
        query_model_runs(
            connection,
            &TurnSessionKey {
                environment_key: "native",
                agent: "opencode",
                session_id: "bounded",
            },
            &FenceScope::single(1),
        )
        .unwrap()
    });
    assert_eq!(runs.len(), 32);
    assert_eq!(
        runs.into_iter().collect::<std::collections::BTreeSet<_>>(),
        metrics.metrics().model_runs.into_iter().collect()
    );
}

#[test]
fn capture_baseline_returns_closed_typed_causes_and_keeps_io_errors() {
    use antiburn_local::analysis::{
        OpenCodeV2SnapshotError as Error, read_capture_baseline_for_scope,
    };
    let (directory, path, _) = fixture();
    let scopes = vec![("ses_accounting".to_owned(), "/repo".to_owned())];
    let check =
        |error: anyhow::Error, expected| assert_eq!(error.downcast_ref::<Error>(), Some(&expected));
    check(
        read_capture_baseline_for_scope(&path, SourceFormat::OpenCodeSqliteV1, &scopes, &|| false)
            .unwrap_err(),
        Error::UnsupportedSource,
    );
    check(
        read_capture_baseline_for_scope(
            &path,
            SourceFormat::OpenCodeSqliteV2,
            &[scopes[0].clone(), scopes[0].clone()],
            &|| false,
        )
        .unwrap_err(),
        Error::ScopeChanged,
    );
    let too_many = (0..=antiburn_local::analysis::opencode_v2_capture::MAX_CAPTURE_SESSIONS)
        .map(|index| (format!("ses_{index}"), "/repo".to_owned()))
        .collect::<Vec<_>>();
    check(
        read_capture_baseline_for_scope(&path, SourceFormat::OpenCodeSqliteV2, &too_many, &|| {
            false
        })
        .unwrap_err(),
        Error::SourceLimit,
    );
    check(
        read_capture_baseline_for_scope(&path, SourceFormat::OpenCodeSqliteV2, &[], &|| true)
            .unwrap_err(),
        Error::Cancelled,
    );
    check(
        read_capture_baseline_for_scope(&path, SourceFormat::OpenCodeSqliteV2, &scopes, &|| false)
            .unwrap_err(),
        Error::UnsupportedSchema,
    );
    Connection::open(&path).unwrap().execute_batch("CREATE TABLE event_sequence (aggregate_id TEXT PRIMARY KEY NOT NULL,seq INTEGER NOT NULL);
        ALTER TABLE session_v2 ADD COLUMN fork_session_id TEXT;").unwrap();
    check(
        read_capture_baseline_for_scope(
            &path,
            SourceFormat::OpenCodeSqliteV2,
            &[("ses_missing".into(), "/repo".into())],
            &|| false,
        )
        .unwrap_err(),
        Error::ScopeChanged,
    );
    check(
        read_capture_baseline_for_scope(
            &path,
            SourceFormat::OpenCodeSqliteV2,
            &[(scopes[0].0.clone(), "/other".into())],
            &|| false,
        )
        .unwrap_err(),
        Error::ScopeChanged,
    );
    let io_error = read_capture_baseline_for_scope(
        &directory.path().join("missing.db"),
        SourceFormat::OpenCodeSqliteV2,
        &scopes,
        &|| false,
    )
    .unwrap_err();
    assert!(io_error.downcast_ref::<rusqlite::Error>().is_some());
    assert!(
        !io_error
            .to_string()
            .contains(&directory.path().to_string_lossy().to_string())
    );
}
