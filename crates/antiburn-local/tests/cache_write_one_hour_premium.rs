//! Characterization test for the one-hour prompt-cache write premium.
//!
//! Anthropic bills a one-hour prompt-cache write at 2x the input rate,
//! versus 1.25x for the catalogue's default (five-minute) rate. A Claude
//! Code transcript reports this split as a nested `cache_creation` object
//! (`ephemeral_1h_input_tokens`, `ephemeral_5m_input_tokens`) alongside the
//! flat `cache_creation_input_tokens` total. This fixture's one assistant
//! turn writes its whole cache-creation total as a one-hour write, so
//! `SessionMetrics.cost.cache_write_usd` must price every one of those
//! tokens at `input_rate * 2.0`, not the catalogue's default rate.
//!
//! `run_fixture_and_replay` mirrors `turn_row_replay_parity.rs`'s own
//! helper of the same shape: it streams the fixture once through the live
//! pipeline (`SessionMetricsAccumulator` plus `TurnRowSink`), then rebuilds
//! `SessionMetrics` a second time from the `turn` rows that pipeline wrote,
//! and returns both. The two must agree on `cost.cache_write_usd` — the
//! turn row's own `cache_write_1h_tokens` column is the only way the
//! row-replay path can reconstruct the premium.

use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, FenceScope, MemoryTurnRowStore, RawSource, RecordSink,
    SessionEvidenceAccumulator, SessionInput, SessionMetrics, SessionMetricsAccumulator,
    SessionSummary, SourceCapabilities, SourceKind, TurnRowSink, TurnRowStore, TurnSessionKey,
    metrics_from_rows, query_turn_rows, reader_for,
};

#[path = "support/pricing.rs"]
mod pricing;

/// One assistant turn: 100 fresh input tokens, 10 output tokens, and 1,000
/// cache-creation tokens entirely classified as one-hour writes (the
/// nested breakdown reports 0 five-minute tokens).
const FIXTURE: &str = concat!(
    r#"{"type":"assistant","timestamp":"2026-09-13T10:00:00Z","message":{"#,
    r#""id":"msg-1h-1","role":"assistant","model":"claude-opus-4-6","#,
    r#""usage":{"input_tokens":100,"output_tokens":10,"#,
    r#""cache_creation_input_tokens":1000,"#,
    r#""cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":1000}},"#,
    r#""content":[{"type":"text","text":"one-hour cache write"}]}}"#,
    "\n"
);

fn input() -> SessionInput {
    SessionInput {
        agent: "claude".to_string(),
        session_id: "one-hour-premium".to_string(),
        source: RawSource::Jsonl(FIXTURE.to_string()),
        fork_parent_session_id: None,
    }
}

/// A [`RecordSink`] that keeps only the [`SessionSummary`] `finish` hands
/// it, so a second, deterministic run of the same adapter over the same
/// bytes recovers the exact summary the live run also used. Mirrors
/// `turn_row_replay_parity.rs`'s `SummaryCapture`.
#[derive(Default)]
struct SummaryCapture(Option<SessionSummary>);

impl RecordSink for SummaryCapture {
    fn record(&mut self, _record: antiburn_local::analysis::NormalizedRecord) {}

    fn finish(&mut self, summary: SessionSummary) {
        self.0 = Some(summary);
    }
}

/// Streams `input` through the real adapter and the real turn-row
/// pipeline, then rebuilds `SessionMetrics` from the rows that pipeline
/// wrote. Returns `(live, replayed)`.
fn run_fixture_and_replay(input: &SessionInput) -> (SessionMetrics, SessionMetrics) {
    let metrics = SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::from(&input.source),
        capabilities: SourceCapabilities::claude(),
    });
    let store = MemoryTurnRowStore::new(input.agent.clone(), input.session_id.clone());
    let turn_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        None,
    );
    let mut composite = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = reader_for(&input.agent)
        .visit(input, &mut composite)
        .expect("fixture must stream");
    composite.observe_source_outcome(outcome);
    assert!(
        !composite.turn_row_write_failed(),
        "turn row write must not fail"
    );
    let live = composite
        .metrics()
        .expect("finished source must publish metrics");

    let key = TurnSessionKey {
        environment_key: "native",
        agent: &input.agent,
        session_id: &input.session_id,
    };
    let rows = store.with_connection(|conn| {
        query_turn_rows(conn, &key, &FenceScope::single(1)).expect("query rows must succeed")
    });

    let mut capture = SummaryCapture::default();
    reader_for(&input.agent)
        .visit(input, &mut capture)
        .expect("fixture must stream for summary capture");
    let mut summary = capture.0;
    let replayed = metrics_from_rows(
        &input.agent,
        input.session_id.clone(),
        &rows,
        |source_key| {
            summary.take().unwrap_or_else(|| {
                panic!(
                    "metrics_from_rows asked for a second source's summary ({source_key:?}), \
                 but this fixture streams through one source"
                )
            })
        },
    )
    .expect("metrics_from_rows must replay a single source's own rows");

    (live, replayed)
}

#[test]
fn one_hour_cache_writes_price_at_double_the_input_rate_live_and_replayed() {
    pricing::install();
    let input = input();
    let (live, replayed) = run_fixture_and_replay(&input);

    let price = antiburn_local::analysis::lookup_pricing("claude-opus-4-6")
        .expect("the fixture pricing table must price claude-opus-4-6");
    let expected_cache_write_usd = 1_000.0 * price.input_cost_per_token * 2.0;

    let live_cost = live.cost.expect("a fully-priced fixture must have a cost");
    assert!(
        (live_cost.cache_write_usd - expected_cache_write_usd).abs() < 1e-9,
        "live cache_write_usd {} != expected {}",
        live_cost.cache_write_usd,
        expected_cache_write_usd
    );

    let replayed_cost = replayed
        .cost
        .expect("the row-replay path must also price the session");
    assert!(
        (replayed_cost.cache_write_usd - expected_cache_write_usd).abs() < 1e-9,
        "replayed cache_write_usd {} != expected {}",
        replayed_cost.cache_write_usd,
        expected_cache_write_usd
    );
    assert_eq!(live_cost, replayed_cost);
}
