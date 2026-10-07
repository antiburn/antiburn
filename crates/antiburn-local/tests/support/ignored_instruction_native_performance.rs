use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use antiburn_local::analysis::ignored_instructions::{
    AssessmentInput, IgnoredInstructionsCheck, InstructionProvenance, InstructionScope,
    build_jev_context, prepare_session_content, select_session_content, snapshot_from_text,
};
use antiburn_local::analysis::jev::{JevInputField, MAX_REQUEST_BYTES, pack_work_items};
use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, FenceScope, JevCheck, MemoryTurnRowStore, ModelRun, RawSource,
    SessionCoverageRecord, SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator,
    SourceCapabilities, SourceFormat, SourceKind, StoredResume, TurnFacts, TurnRow, TurnRowError,
    TurnRowSink, TurnRowStore, TurnSessionKey, query_turn_content_offset_selected, reader_for,
};
use antiburn_local::pricing::ModelTokens;
use serde_json::{Value, json};

const MAX_CONTENT_QUERY_PARTS: usize = 256;
const MAX_CONTENT_QUERY_BYTES: usize = 1024 * 1024;

struct TimedStore {
    inner: Arc<MemoryTurnRowStore>,
    write_us: AtomicUsize,
    writes: AtomicUsize,
}

impl TurnRowStore for TimedStore {
    fn write_turn_rows(&self, rows: &[TurnRow]) -> Result<(), TurnRowError> {
        let started = Instant::now();
        let result = self.inner.write_turn_rows(rows);
        self.write_us.fetch_add(
            usize::try_from(started.elapsed().as_micros()).unwrap(),
            Ordering::Relaxed,
        );
        self.writes.fetch_add(1, Ordering::Relaxed);
        result
    }
    fn write_coverage_record(&self, record: &SessionCoverageRecord) -> Result<(), TurnRowError> {
        let started = Instant::now();
        let result = self.inner.write_coverage_record(record);
        self.write_us.fetch_add(
            usize::try_from(started.elapsed().as_micros()).unwrap(),
            Ordering::Relaxed,
        );
        self.writes.fetch_add(1, Ordering::Relaxed);
        result
    }
    fn query_turn_facts(&self) -> Result<TurnFacts, TurnRowError> {
        self.inner.query_turn_facts()
    }
    fn query_model_breakdown(&self) -> Result<BTreeMap<String, ModelTokens>, TurnRowError> {
        self.inner.query_model_breakdown()
    }
    fn query_model_runs(&self) -> Result<Vec<ModelRun>, TurnRowError> {
        self.inner.query_model_runs()
    }
    fn query_coverage_record(&self) -> Result<Option<SessionCoverageRecord>, TurnRowError> {
        self.inner.query_coverage_record()
    }
    fn read_resume(&self, key: &str) -> Result<Option<StoredResume>, TurnRowError> {
        self.inner.read_resume(key)
    }
    fn write_resume(&self, key: &str, resume: StoredResume) -> Result<(), TurnRowError> {
        self.inner.write_resume(key, resume)
    }
    fn drop_resume(&self, key: &str) -> Result<(), TurnRowError> {
        self.inner.drop_resume(key)
    }
    fn delete_rows_for_source(&self, key: &str) -> Result<(), TurnRowError> {
        self.inner.delete_rows_for_source(key)
    }
    fn note_resumed_source(&self, key: &str) {
        self.inner.note_resumed_source(key);
    }
}

fn fixture(actions: usize, output_bytes: usize) -> SessionInput {
    let mut text = String::new();
    for index in 0..actions {
        let record = if output_bytes == 0 {
            json!({"type":"assistant","uuid":format!("a{index}"),"message":{"role":"assistant","model":"synthetic-model",
                "content":[{"type":"text","text":"I requested git status."}]}})
        } else {
            json!({"type":"assistant","uuid":format!("a{index}"),"message":{"role":"assistant","model":"synthetic-model",
                "content":[{"type":"thinking","thinking":"PRIVATE_NATIVE_THINKING"},{"type":"tool_use","id":format!("call{index}"),"name":"Bash","input":{"command":"git status"}}]}})
        };
        text.push_str(&record.to_string());
        text.push('\n');
        if output_bytes > 0 {
            text.push_str(&json!({"type":"user","uuid":format!("r{index}"),"parentUuid":format!("a{index}"),
                "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":format!("call{index}"),
                    "content":"SELECTED_OUTPUT ".repeat(output_bytes.div_ceil(16)),"is_error":false}]}}).to_string());
            text.push('\n');
        }
    }
    SessionInput {
        agent: "claude".to_owned(),
        session_id: "synthetic-native-performance".to_owned(),
        source: RawSource::Jsonl(text),
        source_format: SourceFormat::ClaudeJsonl,
        fork_parent_session_id: None,
    }
}

fn sample(input: &SessionInput, actions: usize, output_bytes: usize) -> Value {
    let store = Arc::new(TimedStore {
        inner: MemoryTurnRowStore::new(&input.agent, &input.session_id),
        write_us: AtomicUsize::new(0),
        writes: AtomicUsize::new(0),
    });
    let rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        None,
    );
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::from(&input.source),
        capabilities: SourceCapabilities::claude(),
    });
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new(&input.agent, &input.session_id),
        evidence,
        rows,
    );
    let started = Instant::now();
    let outcome = reader_for(&input.agent).visit(input, &mut sink).unwrap();
    sink.observe_source_outcome(outcome);
    let reader_us = started.elapsed().as_micros();
    assert!(!sink.turn_row_write_failed());
    let key = TurnSessionKey {
        environment_key: "native",
        agent: &input.agent,
        session_id: &input.session_id,
    };
    let mut query_times = Vec::new();
    let mut normalization_us = 0;
    let mut projection_us = 0;
    let mut selected_bytes = 0;
    let mut selected_parts = 0;
    let mut largest_request_bytes = 0;
    for _ in 0..2 {
        let started = Instant::now();
        let content = store.inner.with_connection(|connection| {
            query_turn_content_offset_selected(
                connection,
                &key,
                &FenceScope::single(1),
                None,
                &BTreeMap::new(),
                0,
                IgnoredInstructionsCheck.input_selection(),
            )
            .unwrap()
        });
        query_times.push(started.elapsed().as_micros());
        assert!(content.parts.len() <= MAX_CONTENT_QUERY_PARTS);
        assert_eq!(
            content.coverage.bytes_capped,
            output_bytes > 0 && actions * output_bytes > MAX_CONTENT_QUERY_BYTES
        );
        let started = Instant::now();
        let prepared =
            prepare_session_content(&input.session_id, input.source_format, content, Vec::new());
        normalization_us += started.elapsed().as_micros();
        let started = Instant::now();
        let mut selected =
            select_session_content(&prepared, IgnoredInstructionsCheck.input_selection());
        projection_us += started.elapsed().as_micros();
        assert!(
            !serde_json::to_string(&selected)
                .unwrap()
                .contains("PRIVATE_NATIVE_THINKING")
        );
        selected_bytes = selected
            .actions
            .iter()
            .map(|event| event.text.len())
            .sum::<usize>();
        selected_parts = selected.actions.len();
        assert!(selected_bytes <= MAX_CONTENT_QUERY_BYTES);
        assert!(selected_parts <= MAX_CONTENT_QUERY_PARTS);
        let expected_text = if output_bytes == 0 {
            "I requested git status."
        } else {
            "git status"
        };
        if output_bytes == 0 {
            assert_eq!(selected_parts, actions.min(MAX_CONTENT_QUERY_PARTS));
            assert!(
                selected
                    .actions
                    .iter()
                    .all(|event| event.text == expected_text)
            );
            assert_eq!(selected_bytes, selected_parts * expected_text.len());
        } else {
            let outputs = selected
                .actions
                .iter()
                .filter(|event| event.kind == "tool_result")
                .collect::<Vec<_>>();
            assert!(!outputs.is_empty());
            assert!(outputs.iter().all(|event| event.authority == "tool"
                && event.text == "SELECTED_OUTPUT ".repeat(output_bytes.div_ceil(16))));
            assert!(
                selected
                    .actions
                    .iter()
                    .filter(|event| event.kind == "tool_input")
                    .all(|event| event.text == expected_text)
            );
            assert!(selected_parts < actions * 2);
            selected.instructions = vec![
                snapshot_from_text(
                    "AGENTS.md",
                    "Do not force push.".to_owned(),
                    InstructionProvenance::RecordedInjection,
                    InstructionScope::Project,
                )
                .unwrap(),
            ];
            let context = build_jev_context(&AssessmentInput {
                prior_history_complete: selected.complete,
                content: selected.clone(),
                activity_after_ms: None,
                boundary_positions: BTreeMap::new(),
                source_generation: 1,
                source_fingerprint: None,
                incarnation: 1,
                comparison_after: None,
            })
            .unwrap();
            for output in outputs {
                assert_eq!(
                    context
                        .evidence_store
                        .get(&output.reference.id, JevInputField::BashCommandOutput),
                    Some(output.text.as_str())
                );
            }
            let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
            let packing = pack_work_items(&plan.work_items);
            assert!(packing.skipped_item_ids.is_empty());
            assert!(!packing.batches.is_empty());
            largest_request_bytes = packing
                .batches
                .iter()
                .map(|batch| batch.serialized_bytes)
                .max()
                .unwrap();
            assert!(largest_request_bytes <= MAX_REQUEST_BYTES);
        }
    }
    let write_us = store.write_us.load(Ordering::Relaxed);
    json!({"reader_and_sink_us":reader_us,"row_and_coverage_write_us":write_us,"writes":store.writes.load(Ordering::Relaxed),
        "reader_without_writes_us":reader_us.saturating_sub(write_us as u128),"query_first_us":query_times[0],"query_repeat_us":query_times[1],
        "content_normalization_us_mean":normalization_us/2,"projection_us_mean":projection_us/2,"selected_bytes":selected_bytes,"selected_parts":selected_parts,"largest_request_bytes":largest_request_bytes})
}

pub fn run() -> Value {
    let mut rows = Vec::new();
    for (name, actions, output_bytes) in [
        ("native_small", 16, 0),
        ("native_medium", 128, 0),
        ("native_large", 1024, 0),
        ("native_output_heavy", 16, 256 * 1024),
    ] {
        let input = fixture(actions, output_bytes);
        sample(&input, actions, output_bytes);
        let samples = (0..7)
            .map(|_| sample(&input, actions, output_bytes))
            .collect::<Vec<_>>();
        let mut medians = BTreeMap::new();
        for field in samples[0].as_object().unwrap().keys() {
            let mut values = samples
                .iter()
                .map(|sample| sample[field].as_u64().unwrap())
                .collect::<Vec<_>>();
            values.sort_unstable();
            medians.insert(field.clone(), values[values.len() / 2]);
        }
        rows.push(json!({"name":name,"native_actions":actions,"output_bytes_per_action":output_bytes,"medians":medians}));
    }
    json!({"mode":"release","warmup":1,"samples":7,"rows":rows,
        "boundary":"Real Claude reader, composite sink, SQLite MemoryTurnRowStore writes, fenced selected query, normalization, and projection. Store setup is outside timing. Query first/repeat is not a desktop cache measurement. The large source is measured through its first bounded selected page. Reader-without-writes combines native decoding, source normalization, and non-write sink work; it is not an isolated raw-JSON decoder timing."})
}

pub fn verify_selected_query() {
    for output_bytes in [0, 256 * 1024] {
        let input = fixture(16, output_bytes);
        let measured = sample(&input, 16, output_bytes);
        assert!(measured["writes"].as_u64().unwrap() > 0);
        if output_bytes == 0 {
            assert_eq!(measured["selected_parts"], 16);
        } else {
            assert!(measured["selected_bytes"].as_u64().unwrap() >= output_bytes as u64);
            assert!(measured["selected_parts"].as_u64().unwrap() < 32);
            assert!(measured["largest_request_bytes"].as_u64().unwrap() > 0);
        }
    }
}
