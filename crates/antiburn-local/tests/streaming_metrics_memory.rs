use std::io::Cursor;

use antiburn_local::analysis::{
    BoundedJsonlReader, MAX_RECORD_BYTES, RawSource, SCAN_QUANTUM_BYTES, SessionInput,
    SessionMetricsAccumulator, adapter_for,
};

fn synthetic_transcript(record_count: usize) -> String {
    let mut source = String::with_capacity(record_count * 280);
    for index in 0..record_count {
        source.push_str(&format!(
            "{{\"type\":\"assistant\",\"timestamp\":{},\"message\":{{\"id\":\"synthetic-{index}\",\"role\":\"assistant\",\"model\":\"claude-3-5-haiku-20241022\",\"usage\":{{\"input_tokens\":2,\"output_tokens\":3}},\"content\":[{{\"type\":\"text\",\"text\":\"Synthetic turn {index}.\"}}]}}}}\n",
            1_760_000_000 + index
        ));
    }
    source
}

#[test]
fn retained_state_grows_only_with_metric_bearing_records() {
    const RECORD_COUNT: usize = 40_000;
    const RETAINED_BYTES_PER_RECORD_BOUND: usize = 1_024;

    let source = synthetic_transcript(RECORD_COUNT);
    assert!(source.len() > MAX_RECORD_BYTES);
    let input = SessionInput {
        agent: "claude".to_string(),
        session_id: "synthetic-memory-measurement".to_string(),
        source: RawSource::Jsonl(source.clone()),
    };
    let mut accumulator = SessionMetricsAccumulator::new(&input.agent, &input.session_id);
    adapter_for("claude")
        .visit(&input, &mut accumulator)
        .expect("synthetic source must stream");

    assert_eq!(accumulator.retained_turns(), RECORD_COUNT);
    assert!(
        accumulator.retained_bytes() < RECORD_COUNT * RETAINED_BYTES_PER_RECORD_BOUND,
        "retained {} bytes for {RECORD_COUNT} records",
        accumulator.retained_bytes()
    );

    let mut reader = BoundedJsonlReader::new(Cursor::new(source.as_bytes()));
    while reader.next_record(&|| false).is_some() {}
    assert!(reader.retained_record_bytes_high_water() <= SCAN_QUANTUM_BYTES * 4);
}
