use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, RawSource, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SourceCapabilities, SourceKind, adapter_for,
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
    let mut composite = CompositeSink::new(metrics, evidence);
    let outcome = adapter_for("claude")
        .visit(&input, &mut composite)
        .expect("synthetic Claude fixture must stream");
    composite.observe_source_outcome(outcome);
    composite
}
