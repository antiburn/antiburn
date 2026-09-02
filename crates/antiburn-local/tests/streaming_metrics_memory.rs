#[path = "support/corpus.rs"]
mod corpus;

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use antiburn_local::analysis::{
    BoundedJsonlReader, ClaudeAdapter, CompositeSink, ContextSourceKind, EvidenceObservation,
    EvidenceSnapshot, EvidenceSource, MemoryTurnRowStore, NormalizedEvent, NormalizedRecord,
    RESUME_SNAPSHOT_REVISION, RETAINED_EVIDENCE_BYTES_BOUND, RETAINED_METRICS_BYTES_BOUND,
    RawSource, RecordSink, RelationProvenance, ResumePoint, Role, SCAN_QUANTUM_BYTES,
    SessionCoverageRecord, SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator,
    SessionSummary, SourceCapabilities, SourceClaim, SourceKind, StoredResume, StreamSnapshot,
    TURN_ROW_BATCH_SIZE, ToolCall, TurnFacts, TurnRow, TurnRowError, TurnRowSink, TurnRowStore,
    TurnScope, TurnSessionKey, Usage, adapter_for, count_turn_content_rows, count_turn_rows,
    merge_metrics,
};
use antiburn_local::discovery::source_version::head_hash_of;
use antiburn_local::discovery::{FingerprintInputs, SourceStat};
use corpus::{SessionSpec, generate_session, generate_session_of_bytes};

#[test]
fn streamed_corpus_keeps_framing_and_metrics_bounded() {
    let session = generate_session_of_bytes(401, 0, 3 * 1024 * 1024);
    let input = SessionInput {
        agent: "claude".to_string(),
        session_id: session.session_id,
        source: RawSource::Jsonl(session.jsonl.clone()),
    };
    let mut accumulator = SessionMetricsAccumulator::new(&input.agent, &input.session_id);
    adapter_for("claude")
        .visit(&input, &mut accumulator)
        .expect("synthetic source streams");
    assert!(accumulator.retained_bytes() <= RETAINED_METRICS_BYTES_BOUND);

    let mut reader = BoundedJsonlReader::new(Cursor::new(session.jsonl.as_bytes()));
    while reader.next_record(&|| false).is_some() {}
    assert!(reader.retained_record_bytes_high_water() <= SCAN_QUANTUM_BYTES * 4);
}

fn saturated_accumulator(record_count: usize) -> SessionMetricsAccumulator {
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "bounded-memory");
    for index in 0..record_count {
        let mut event = NormalizedEvent::new(Role::Assistant);
        let timestamp = if index < 1_100 {
            i64::try_from(index).expect("index fits") * 600_000
        } else {
            1_100 * 600_000 + i64::try_from(index - 1_100).expect("index fits") * 1_000
        };
        event.ts_ms = Some(timestamp);
        event.model = Some(format!("synthetic-model-{}", index.min(100)));
        event.thinking_mode = Some(format!("mode-{}", index.min(40)));
        event.speed = Some(format!("speed-{}", index.min(20)));
        event.is_compaction_boundary = index.is_multiple_of(97);
        event.usage = Usage {
            input_tokens: 2,
            output_tokens: 3,
            cache_read_tokens: 20_000,
            cache_creation_tokens: 2,
        };
        event.tools.push(ToolCall::new("Skill"));
        event.tools[0].detail = Some(format!("synthetic-skill-{}", index.min(300)));
        event.tools.push(ToolCall::new(format!(
            "mcp__synthetic-server-{}__search",
            index.min(200)
        )));
        event.may_resolve_late_tool = true;
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    }
    accumulator.finish(SessionSummary::default());
    accumulator
}

#[test]
fn retained_state_stays_bounded_near_the_exact_turn_threshold() {
    let before = saturated_accumulator(540);
    let after = saturated_accumulator(541);
    assert!(before.retained_bytes() <= RETAINED_METRICS_BYTES_BOUND);
    assert!(after.retained_bytes() <= RETAINED_METRICS_BYTES_BOUND);
}

#[test]
fn retained_state_stops_growing_once_saturated() {
    let at_40k = saturated_accumulator(40_000);
    let at_400k = saturated_accumulator(400_000);
    assert_eq!(at_40k.observed_turns(), 40_000);
    assert_eq!(at_400k.observed_turns(), 400_000);
    assert!(
        at_40k.retained_bytes().abs_diff(at_400k.retained_bytes()) <= 32 * 1_024,
        "retained state varied from {} to {} bytes",
        at_40k.retained_bytes(),
        at_400k.retained_bytes()
    );
    assert!(
        at_400k.retained_bytes() <= RETAINED_METRICS_BYTES_BOUND,
        "retained {} bytes",
        at_400k.retained_bytes()
    );
}

#[test]
fn retained_state_is_bounded_for_a_name_flood() {
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "name-flood");
    for index in 0..5_000 {
        let mut event = NormalizedEvent::new(Role::Assistant);
        event.ts_ms = Some(index as i64);
        event.model = Some(format!("synthetic-model-{index}"));
        event.usage.output_tokens = 1;
        event
            .tools
            .push(ToolCall::new(format!("synthetic-tool-{index}")));
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    }
    accumulator.finish(SessionSummary::default());
    assert!(
        accumulator.retained_bytes() <= RETAINED_METRICS_BYTES_BOUND,
        "retained {} bytes",
        accumulator.retained_bytes()
    );
}

#[test]
fn retained_state_stays_small_for_a_small_session() {
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "small-subagent");
    for index in 0..50 {
        let mut event = NormalizedEvent::new(Role::Assistant);
        event.ts_ms = Some(index);
        event.usage.output_tokens = 1;
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    }
    accumulator.finish(SessionSummary::default());
    assert!(accumulator.retained_bytes() <= 32 * 1_024);
}

/// Records only the largest batch and the total row count it ever saw, so
/// the assertions below need no real database.
#[derive(Default)]
struct CountingWriter {
    max_batch: AtomicUsize,
    total_rows: AtomicUsize,
}

impl TurnRowStore for CountingWriter {
    fn write_turn_rows(&self, rows: &[TurnRow]) -> Result<(), TurnRowError> {
        self.max_batch.fetch_max(rows.len(), Ordering::SeqCst);
        self.total_rows.fetch_add(rows.len(), Ordering::SeqCst);
        Ok(())
    }

    // Never read: this test only asserts on the batches the sink wrote.
    fn query_turn_facts(&self) -> Result<TurnFacts, TurnRowError> {
        Err(TurnRowError("not readable".to_owned()))
    }

    fn query_model_breakdown(
        &self,
    ) -> Result<
        std::collections::BTreeMap<String, antiburn_local::pricing::ModelTokens>,
        TurnRowError,
    > {
        Err(TurnRowError("not readable".to_owned()))
    }

    fn query_model_runs(&self) -> Result<Vec<antiburn_local::analysis::ModelRun>, TurnRowError> {
        Err(TurnRowError("not readable".to_owned()))
    }

    fn write_coverage_record(&self, _record: &SessionCoverageRecord) -> Result<(), TurnRowError> {
        Ok(())
    }

    fn query_coverage_record(&self) -> Result<Option<SessionCoverageRecord>, TurnRowError> {
        Err(TurnRowError("not readable".to_owned()))
    }

    fn read_resume(&self, _source_key: &str) -> Result<Option<StoredResume>, TurnRowError> {
        Err(TurnRowError("not readable".to_owned()))
    }

    fn write_resume(&self, _source_key: &str, _resume: StoredResume) -> Result<(), TurnRowError> {
        Ok(())
    }

    fn drop_resume(&self, _source_key: &str) -> Result<(), TurnRowError> {
        Ok(())
    }

    fn delete_rows_for_source(&self, _source_key: &str) -> Result<(), TurnRowError> {
        Ok(())
    }
}

#[test]
fn the_turn_row_sink_stays_bounded_over_a_streamed_corpus() {
    let session = generate_session_of_bytes(402, 0, 3 * 1024 * 1024);
    let input = SessionInput {
        agent: "claude".to_string(),
        session_id: session.session_id.clone(),
        source: RawSource::Jsonl(session.jsonl.clone()),
    };
    let writer = Arc::new(CountingWriter::default());
    let metrics = SessionMetricsAccumulator::new(&input.agent, &input.session_id);
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    });
    let sink = TurnRowSink::new(
        Arc::clone(&writer) as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        None,
    );
    let mut composite = CompositeSink::with_turn_rows(metrics, evidence, sink);
    adapter_for("claude")
        .visit(&input, &mut composite)
        .expect("synthetic source streams");

    assert!(!composite.turn_row_write_failed());
    assert!(
        writer.max_batch.load(Ordering::SeqCst) <= TURN_ROW_BATCH_SIZE,
        "largest batch was {} rows, batch size is {}",
        writer.max_batch.load(Ordering::SeqCst),
        TURN_ROW_BATCH_SIZE
    );
    assert!(writer.total_rows.load(Ordering::SeqCst) > 0);
}

/// Builds an evidence accumulator and floods every capped collection
/// (`tools`, `context_sources`, `subagents`, and `diagnostics.
/// unrecognized_types`) with `record_count` distinct entries. Every name
/// is unique, so a bound this reaches proves the caps hold, not that the
/// flood happened to repeat a name.
fn saturated_evidence_accumulator(record_count: usize) -> SessionEvidenceAccumulator {
    let mut accumulator = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "synthetic".to_owned(),
        session_id: "bounded-evidence".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    });
    for index in 0..record_count {
        let mut event = NormalizedEvent::new(Role::Assistant);
        event.ts_ms = Some(index as i64);
        event
            .tools
            .push(ToolCall::new(format!("synthetic-tool-{index}")));
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::ContextSource {
                kind: ContextSourceKind::Skill,
                name: format!("synthetic-skill-{index}"),
                description: Some(format!("Synthetic skill description {index}.")),
            },
        )));
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::ContextSource {
                kind: ContextSourceKind::McpServer,
                name: format!("synthetic-mcp-{index}"),
                description: None,
            },
        )));
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::SubagentSpawn {
                ts_ms: Some(index as i64),
                parent_model: Some(format!("synthetic-model-{index}")),
                provenance: RelationProvenance::TaskToolUse,
            },
        )));
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::UnrecognizedType {
                discriminator: format!("synthetic-unrecognized-{index}"),
                inert: true,
            },
        )));
    }
    accumulator.finish(SessionSummary::default());
    accumulator
}

#[test]
fn evidence_retained_state_is_bounded_for_a_name_flood() {
    let accumulator = saturated_evidence_accumulator(5_000);
    assert!(
        accumulator.retained_bytes() <= RETAINED_EVIDENCE_BYTES_BOUND,
        "retained {} bytes",
        accumulator.retained_bytes()
    );
}

#[test]
fn evidence_retained_state_stops_growing_once_saturated() {
    let at_40k = saturated_evidence_accumulator(40_000);
    let at_400k = saturated_evidence_accumulator(400_000);
    assert!(
        at_40k.retained_bytes().abs_diff(at_400k.retained_bytes()) <= 4 * 1_024,
        "retained state varied from {} to {} bytes",
        at_40k.retained_bytes(),
        at_400k.retained_bytes()
    );
    assert!(
        at_400k.retained_bytes() <= RETAINED_EVIDENCE_BYTES_BOUND,
        "retained {} bytes",
        at_400k.retained_bytes()
    );
}

/// Streams one [`SessionInput`] through a fresh metrics accumulator, an
/// evidence accumulator, and a [`TurnRowSink`] against `store`, the same
/// way production ingest streams one source. `scope` forces
/// [`TurnScope::Delegated`] for a child; `None` keeps the parent's derived
/// scope.
fn stream_into(
    input: &SessionInput,
    store: &Arc<MemoryTurnRowStore>,
    scope: Option<TurnScope>,
) -> (SessionMetricsAccumulator, SessionEvidenceAccumulator) {
    let metrics = SessionMetricsAccumulator::new(&input.agent, &input.session_id);
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    });
    let turn_rows = TurnRowSink::new(
        Arc::clone(store) as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        scope,
    );
    let mut composite = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = adapter_for("claude")
        .visit(input, &mut composite)
        .expect("synthetic source must stream");
    composite.observe_source_outcome(outcome);
    composite
        .into_parts()
        .expect("a finished synthetic pass must publish")
}

#[test]
fn a_parent_and_thirty_children_keep_every_accumulator_bounded() {
    const CHILD_COUNT: usize = 30;

    let mut parent_spec = SessionSpec::tier_s(701, 0, 600);
    parent_spec.task_spawns = CHILD_COUNT;
    let parent_session = generate_session(&parent_spec);
    let store = MemoryTurnRowStore::new("claude", parent_session.session_id.clone());
    let parent_input = SessionInput {
        agent: "claude".to_string(),
        session_id: parent_session.session_id.clone(),
        source: RawSource::Jsonl(parent_session.jsonl.clone()),
    };
    let (parent_metrics, mut parent_evidence) = stream_into(&parent_input, &store, None);
    assert!(parent_metrics.retained_bytes() <= RETAINED_METRICS_BYTES_BOUND);
    assert!(parent_evidence.retained_bytes() <= RETAINED_EVIDENCE_BYTES_BOUND);

    let mut child_metrics_list = Vec::with_capacity(CHILD_COUNT);
    for child_index in 0..CHILD_COUNT {
        let mut child_spec = SessionSpec::tier_s(701, child_index + 1, 400);
        child_spec.delegated = true;
        let child_session = generate_session(&child_spec);
        let child_input = SessionInput {
            agent: "claude".to_string(),
            session_id: child_session.session_id.clone(),
            source: RawSource::Jsonl(child_session.jsonl.clone()),
        };
        let (child_metrics, child_evidence) =
            stream_into(&child_input, &store, Some(TurnScope::Delegated));
        assert!(
            child_metrics.retained_bytes() <= RETAINED_METRICS_BYTES_BOUND,
            "child {child_index} metrics retained {} bytes",
            child_metrics.retained_bytes()
        );
        assert!(
            child_evidence.retained_bytes() <= RETAINED_EVIDENCE_BYTES_BOUND,
            "child {child_index} evidence retained {} bytes",
            child_evidence.retained_bytes()
        );
        // The aggregate evidence path: the parent folds each child's
        // coverage as it streams, the way `TurnScope::Delegated` sub-agent
        // discovery does in production.
        parent_evidence.observe_child_coverage(&child_evidence);
        child_metrics_list.push(child_metrics);
    }

    // Folding thirty children's coverage must not push the parent's
    // residual past the same bound a single session observes.
    assert!(
        parent_evidence.retained_bytes() <= RETAINED_EVIDENCE_BYTES_BOUND,
        "parent evidence retained {} bytes after folding {CHILD_COUNT} children",
        parent_evidence.retained_bytes()
    );

    // The aggregate metrics path: `merge_metrics` holds the parent and
    // every child accumulator live at once. Each already proved bounded
    // above; this proves the merge itself completes and reports every
    // child's contribution.
    let merged = merge_metrics(&parent_metrics, &child_metrics_list);
    assert!(merged.event_count > 0);
    assert!(
        merged
            .buckets
            .iter()
            .map(|bucket| bucket.subagent_tokens)
            .sum::<u64>()
            > 0,
        "the merged buckets must carry the children's token contribution"
    );

    let facts = store
        .query_turn_facts()
        .expect("the shared store must answer a facts query");
    assert!(
        facts.delegated_turns > 0,
        "the children's rows must count as delegated turns"
    );
}

/// Forwards every record to `inner` and, before doing so, sums the bytes of
/// every `TurnContent` part it carries. This is the byte count the source
/// actually fed the row sink, independent of how SQLite ends up storing it.
struct ContentByteCountingSink {
    inner: CompositeSink,
    content_bytes: usize,
}

impl RecordSink for ContentByteCountingSink {
    fn record(&mut self, record: NormalizedRecord) {
        if let NormalizedRecord::TurnContent(content) = &record {
            self.content_bytes = self.content_bytes.saturating_add(
                content
                    .parts
                    .iter()
                    .map(|part| part.text.len())
                    .sum::<usize>(),
            );
        }
        self.inner.record(record);
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.inner.finish(summary);
    }
}

/// Forwards every write to a real [`MemoryTurnRowStore`] and separately
/// tallies the row count the sink handed it, so a test can compare that
/// tally against the store's own count without re-deriving it from SQL.
struct CountingRealStore {
    inner: Arc<MemoryTurnRowStore>,
    total_rows: AtomicUsize,
}

impl TurnRowStore for CountingRealStore {
    fn write_turn_rows(&self, rows: &[TurnRow]) -> Result<(), TurnRowError> {
        self.total_rows.fetch_add(rows.len(), Ordering::SeqCst);
        self.inner.write_turn_rows(rows)
    }

    fn query_turn_facts(&self) -> Result<TurnFacts, TurnRowError> {
        self.inner.query_turn_facts()
    }

    fn query_model_breakdown(
        &self,
    ) -> Result<
        std::collections::BTreeMap<String, antiburn_local::pricing::ModelTokens>,
        TurnRowError,
    > {
        self.inner.query_model_breakdown()
    }

    fn query_model_runs(&self) -> Result<Vec<antiburn_local::analysis::ModelRun>, TurnRowError> {
        self.inner.query_model_runs()
    }

    fn write_coverage_record(&self, record: &SessionCoverageRecord) -> Result<(), TurnRowError> {
        self.inner.write_coverage_record(record)
    }

    fn query_coverage_record(&self) -> Result<Option<SessionCoverageRecord>, TurnRowError> {
        self.inner.query_coverage_record()
    }

    fn read_resume(&self, source_key: &str) -> Result<Option<StoredResume>, TurnRowError> {
        self.inner.read_resume(source_key)
    }

    fn write_resume(&self, source_key: &str, resume: StoredResume) -> Result<(), TurnRowError> {
        self.inner.write_resume(source_key, resume)
    }

    fn drop_resume(&self, source_key: &str) -> Result<(), TurnRowError> {
        self.inner.drop_resume(source_key)
    }

    fn delete_rows_for_source(&self, source_key: &str) -> Result<(), TurnRowError> {
        self.inner.delete_rows_for_source(source_key)
    }
}

#[test]
fn disk_bytes_per_row_stay_bounded_with_content_enabled() {
    let session = generate_session_of_bytes(801, 0, 4 * 1024 * 1024);
    let input = SessionInput {
        agent: "claude".to_string(),
        session_id: session.session_id.clone(),
        source: RawSource::Jsonl(session.jsonl.clone()),
    };

    // `MemoryTurnRowStore` is real SQLite (in memory), so `PRAGMA
    // page_count` and `PRAGMA page_size` read its true on-disk shape.
    // Content storage needs no separate switch: every Claude adapter pass
    // emits a `TurnContent` record after each turn's `MetricsEvent`, and
    // `TurnRowSink::observe` always attaches it to the buffered row before
    // any real `TurnRowStore` (unlike the `CountingWriter` test double
    // above) writes it into `turn_content`.
    let inner_store = MemoryTurnRowStore::new(input.agent.clone(), input.session_id.clone());
    let counting_store = Arc::new(CountingRealStore {
        inner: Arc::clone(&inner_store),
        total_rows: AtomicUsize::new(0),
    });
    let metrics = SessionMetricsAccumulator::new(&input.agent, &input.session_id);
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    });
    let turn_rows = TurnRowSink::new(
        Arc::clone(&counting_store) as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        None,
    );
    let composite = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let mut counted = ContentByteCountingSink {
        inner: composite,
        content_bytes: 0,
    };
    let outcome = adapter_for("claude")
        .visit(&input, &mut counted)
        .expect("synthetic source streams");
    counted.inner.observe_source_outcome(outcome);
    assert!(!counted.inner.turn_row_write_failed());

    let key = TurnSessionKey {
        environment_key: "native",
        agent: &input.agent,
        session_id: &input.session_id,
    };
    let (turn_row_count, content_row_count, content_stored_bytes, total_db_bytes) = inner_store
        .with_connection(|conn| {
            let turn_row_count = count_turn_rows(conn, &key, 1).expect("turn rows must count");
            let content_row_count =
                count_turn_content_rows(conn, &key, 1).expect("content rows must count");
            let content_stored_bytes: i64 = conn
                .query_row(
                    "SELECT COALESCE(SUM(LENGTH(content)), 0) FROM turn_content",
                    [],
                    |row| row.get(0),
                )
                .expect("content bytes must sum");
            let page_count: i64 = conn
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .expect("page_count must read");
            let page_size: i64 = conn
                .query_row("PRAGMA page_size", [], |row| row.get(0))
                .expect("page_size must read");
            (
                turn_row_count,
                content_row_count,
                content_stored_bytes.max(0) as u64,
                (page_count * page_size).max(0) as u64,
            )
        });

    assert_eq!(
        counting_store.total_rows.load(Ordering::SeqCst) as u64,
        turn_row_count,
        "the store's row count must match what the sink wrote"
    );
    assert!(turn_row_count > 0);
    assert!(content_row_count > 0);

    // The whole database's bytes, minus the raw content blob bytes it
    // holds, approximates the `turn` table's own footprint (rows plus its
    // index). Content is computed separately below, so it is never
    // counted twice.
    let turn_table_bytes = total_db_bytes.saturating_sub(content_stored_bytes);
    let bytes_per_row = turn_table_bytes / turn_row_count;
    assert!(
        bytes_per_row <= 2_048,
        "turn-table bytes/row was {bytes_per_row} ({turn_table_bytes} bytes over {turn_row_count} rows)"
    );

    assert!(counted.content_bytes > 0);
    assert!(
        content_stored_bytes <= (counted.content_bytes as u64).saturating_mul(2),
        "turn_content stored {content_stored_bytes} bytes for {} fed bytes",
        counted.content_bytes
    );
}

/// An encoded [`StreamSnapshot`] (`StreamSnapshot::encode`, `postcard`) for
/// a full pass over the largest corpus tier this file measures elsewhere
/// (4 MiB, same as [`disk_bytes_per_row_stay_bounded_with_content_enabled`])
/// stays under a documented bound. `RESUMED_SNAPSHOT_BYTES_BOUND` below
/// records the measured size this test found and explains the bound picked
/// from it.
#[test]
fn a_serialized_snapshot_for_the_largest_corpus_tier_stays_bounded() {
    /// Measured: a 4 MiB synthetic session's `StreamSnapshot` encodes to
    /// 520,777 bytes (about 509 KiB) with `postcard`, versus 2,002,215
    /// bytes (about 1.9 MiB) for the same snapshot as JSON — `postcard`'s
    /// compact varint and length-prefixed encoding does not re-expand this
    /// type's interned name IDs and packed slot indices into full field
    /// names and strings the way JSON does. This bound rounds the measured
    /// size up generously (over 2x), the same margin
    /// `RETAINED_EVIDENCE_BYTES_BOUND`'s own doc comment uses.
    const RESUMED_SNAPSHOT_BYTES_BOUND: usize = 1_100 * 1024;

    let session = generate_session_of_bytes(901, 0, 4 * 1024 * 1024);
    let directory = tempfile::TempDir::new().expect("tempdir");
    let path = directory.path().join("session.jsonl");
    std::fs::write(&path, &session.jsonl).expect("write source");
    let input = SessionInput {
        agent: "claude".to_string(),
        session_id: session.session_id.clone(),
        source: RawSource::File(path.clone()),
    };

    let file = std::fs::File::open(&path).expect("open source for claim");
    let stat = SourceStat::from_open_std_file(&file).expect("stat source for claim");
    let claim = SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
        stat,
        head_hash: Some(head_hash_of(session.jsonl.as_bytes())),
    });
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".to_owned(),
        session_id: session.session_id.clone(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    });
    let empty_snapshot = StreamSnapshot {
        revision: RESUME_SNAPSHOT_REVISION,
        resume: ResumePoint {
            offset: 0,
            tail_hash: head_hash_of(&[]),
            tail_len: 0,
        },
        adapter: ClaudeAdapter::empty_adapter_snapshot(),
        metrics: SessionMetricsAccumulator::new("claude", &session.session_id),
        evidence: EvidenceSnapshot {
            record: evidence.coverage_record(),
            resume: Default::default(),
        },
        next_turn_index: 0,
    };

    let store = MemoryTurnRowStore::new("claude", &session.session_id);
    let mut composite = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("claude", &session.session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: session.session_id.clone(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            session.session_id.clone(),
            None,
        ),
    );
    let visit = ClaudeAdapter
        .visit_claimed_resumed(&input, &claim, &empty_snapshot, &|| false, &mut composite)
        .expect("full pass over the largest corpus tier must stream");
    assert_eq!(
        visit.outcome,
        antiburn_local::analysis::VisitOutcome::AcceptedFull
    );
    composite.observe_source_outcome(visit.outcome);
    assert!(!composite.turn_row_write_failed());
    let resume = visit
        .resume
        .expect("a settled pass over a quiescent source carries a resume");
    let snapshot = composite
        .snapshot(resume)
        .expect("full pass must publish to snapshot");

    let encoded = snapshot.encode();
    assert!(
        encoded.len() <= RESUMED_SNAPSHOT_BYTES_BOUND,
        "encoded snapshot was {} bytes, bound is {RESUMED_SNAPSHOT_BYTES_BOUND}",
        encoded.len()
    );
}

/// An encoded [`StreamSnapshot`] for a full pass over the largest Codex
/// characterization fixture stays under a documented bound.
///
/// `corpus.rs` only generates Claude-shaped JSONL — see its module doc — so
/// this test cannot build a Codex-shaped corpus the way
/// [`a_serialized_snapshot_for_the_largest_corpus_tier_stays_bounded`] does
/// for Claude with `generate_session_of_bytes`. It measures the largest
/// Codex characterization fixture instead: `collab_agent_records.jsonl`
/// (5,550 bytes), the largest Codex-shaped session this crate ships.
#[test]
fn a_serialized_codex_snapshot_for_the_largest_fixture_stays_bounded() {
    /// Measured: `collab_agent_records.jsonl`'s `StreamSnapshot` encodes to
    /// 1,335 bytes with `postcard`. This bound rounds the measured size up
    /// generously (over 3x), the same margin `RESUMED_SNAPSHOT_BYTES_BOUND`
    /// above uses for the Claude case.
    const RESUMED_CODEX_SNAPSHOT_BYTES_BOUND: usize = 4 * 1024;

    let jsonl = include_str!("fixtures/codex_characterization/collab_agent_records.jsonl");
    let directory = tempfile::TempDir::new().expect("tempdir");
    let path = directory.path().join("session.jsonl");
    std::fs::write(&path, jsonl).expect("write source");
    let input = SessionInput {
        agent: "codex".to_string(),
        session_id: "codex-snapshot-bound".to_string(),
        source: RawSource::File(path.clone()),
    };

    let file = std::fs::File::open(&path).expect("open source for claim");
    let stat = SourceStat::from_open_std_file(&file).expect("stat source for claim");
    let claim = SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
        stat,
        head_hash: Some(head_hash_of(jsonl.as_bytes())),
    });
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "codex".to_owned(),
        session_id: "codex-snapshot-bound".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::codex(),
    });
    let empty_snapshot = StreamSnapshot {
        revision: RESUME_SNAPSHOT_REVISION,
        resume: ResumePoint {
            offset: 0,
            tail_hash: head_hash_of(&[]),
            tail_len: 0,
        },
        adapter: adapter_for("codex")
            .empty_resume_state()
            .expect("codex adapter must support resume"),
        metrics: SessionMetricsAccumulator::new("codex", "codex-snapshot-bound"),
        evidence: EvidenceSnapshot {
            record: evidence.coverage_record(),
            resume: Default::default(),
        },
        next_turn_index: 0,
    };

    let store = MemoryTurnRowStore::new("codex", "codex-snapshot-bound");
    let mut composite = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("codex", "codex-snapshot-bound"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "codex".to_owned(),
            session_id: "codex-snapshot-bound".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::codex(),
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            "codex-snapshot-bound".to_owned(),
            None,
        ),
    );
    let visit = adapter_for("codex")
        .visit_claimed_resumed(&input, &claim, &empty_snapshot, &|| false, &mut composite)
        .expect("full pass over the largest Codex fixture must stream");
    assert_eq!(
        visit.outcome,
        antiburn_local::analysis::VisitOutcome::AcceptedFull
    );
    composite.observe_source_outcome(visit.outcome);
    assert!(!composite.turn_row_write_failed());
    let resume = visit
        .resume
        .expect("a settled pass over a quiescent source carries a resume");
    let snapshot = composite
        .snapshot(resume)
        .expect("full pass must publish to snapshot");

    let encoded = snapshot.encode();
    assert!(
        encoded.len() <= RESUMED_CODEX_SNAPSHOT_BYTES_BOUND,
        "encoded snapshot was {} bytes, bound is {RESUMED_CODEX_SNAPSHOT_BYTES_BOUND}",
        encoded.len()
    );
}
