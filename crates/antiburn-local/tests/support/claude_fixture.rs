//! Shared by every test that includes it through `#[path]`. Not every
//! binary uses every helper here — the same reason `corpus.rs` allows this.
#![allow(dead_code)]

use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, MemoryTurnRowStore, RawSource, SessionEvidenceAccumulator,
    SessionInput, SessionMetricsAccumulator, SourceCapabilities, SourceKind, TurnRowSink,
    TurnRowStore, adapter_for,
};

pub fn read_fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/claude_characterization/{name}.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).expect("synthetic fixture must be readable")
}

pub fn session_input(name: &str) -> SessionInput {
    SessionInput {
        agent: "claude".to_owned(),
        session_id: name.to_owned(),
        source: RawSource::Jsonl(read_fixture(name)),
    }
}

pub fn stream_composite(name: &str) -> CompositeSink {
    stream_input(session_input(name))
}

pub fn stream_source(name: &str, source: String) -> CompositeSink {
    stream_input(SessionInput {
        agent: "claude".to_owned(),
        session_id: name.to_owned(),
        source: RawSource::Jsonl(source),
    })
}

fn stream_input(input: SessionInput) -> CompositeSink {
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
    let outcome = adapter_for("claude")
        .visit(&input, &mut composite)
        .expect("synthetic Claude fixture must stream");
    composite.observe_source_outcome(outcome);
    composite
}
