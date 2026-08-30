#[path = "support/corpus.rs"]
mod corpus;

use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};

use antiburn_local::analysis::{
    BoundedJsonlReader, CompositeSink, EvidenceSource, NormalizedEvent, NormalizedRecord,
    RETAINED_METRICS_BYTES_BOUND, RawSource, RecordSink, Role, SCAN_QUANTUM_BYTES,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SessionSummary,
    SourceCapabilities, SourceKind, TURN_ROW_BATCH_SIZE, ToolCall, TurnRow, TurnRowSink,
    TurnRowWriteError, TurnRowWriter, Usage, adapter_for,
};
use corpus::generate_session_of_bytes;

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

impl TurnRowWriter for CountingWriter {
    fn write_turn_rows(&self, rows: &[TurnRow]) -> Result<(), TurnRowWriteError> {
        self.max_batch.fetch_max(rows.len(), Ordering::SeqCst);
        self.total_rows.fetch_add(rows.len(), Ordering::SeqCst);
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
    let writer = CountingWriter::default();
    let metrics = SessionMetricsAccumulator::new(&input.agent, &input.session_id);
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    });
    let sink = TurnRowSink::new(&writer, input.session_id.clone());
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
