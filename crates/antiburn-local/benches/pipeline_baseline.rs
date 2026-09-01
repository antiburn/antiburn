//! Pipeline measurement baseline (issue #224).
//!
//! Criterion benches over the synthetic corpus generator shared with
//! `tests/pipeline_corpus.rs`. All sources are generated in memory or into a
//! tempdir; nothing reads a real transcript. The numbers are an indicative
//! local baseline recorded in `benches/BASELINE.md` with machine context —
//! they are not CI-enforced thresholds.
//!
//! Stage coverage (of the ten stages the master plan names): source reading,
//! parsing/framing, normalization, metrics accumulation, evidence
//! accumulation, and report reduction. Discovery, queue wait, persistence,
//! report query, and IPC live in the desktop app and need a desktop-side
//! harness.

#[path = "../tests/support/corpus.rs"]
mod corpus;

use std::hint::black_box;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use antiburn_local::analysis::{
    ANALYZER_REVISION, AppendOnlyGuarantee, BoundedJsonlReader, ClaudeAdapter, CompositeSink,
    EVIDENCE_SCHEMA_REVISION, EvidenceSource, MAX_RECORD_BYTES, MemoryTurnRowStore,
    NormalizedRecord, PARSER_REVISION, RawSource, RecordSink, SessionEvidence,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SessionSummary,
    SourceCapabilities, SourceClaim, SourceKind, TurnRowSink, TurnRowStore, VisitOutcome,
    adapter_for, normalize_source,
};
use antiburn_local::discovery::source_version::{FingerprintInputs, SourceStat, head_hash_of};
use antiburn_local::insights::{
    CoverageBucket, CoverageCounts, EfficiencyReportAccumulator, ReportContext, ReportWindow,
};
use corpus::{
    GeneratedSession, SessionSpec, generate_session, generate_session_of_bytes, write_provider_db,
};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use rusqlite::{Connection, params};
use tempfile::TempDir;

const MIB: usize = 1024 * 1024;
const KIB: usize = 1024;

struct NoopSink;

impl RecordSink for NoopSink {
    fn record(&mut self, _record: NormalizedRecord) {}
    fn finish(&mut self, _summary: SessionSummary) {}
}

fn jsonl_input(session: &GeneratedSession) -> SessionInput {
    SessionInput {
        agent: "claude".to_string(),
        session_id: session.session_id.clone(),
        source: RawSource::Jsonl(session.jsonl.clone()),
    }
}

fn file_input(session_id: &str, path: &Path) -> SessionInput {
    file_input_for("claude", session_id, path)
}

fn file_input_for(agent: &str, session_id: &str, path: &Path) -> SessionInput {
    SessionInput {
        agent: agent.to_string(),
        session_id: session_id.to_string(),
        source: RawSource::File(path.to_path_buf()),
    }
}

fn composite_for(input: &SessionInput) -> CompositeSink {
    let mut capabilities = match input.agent.as_str() {
        "claude" => SourceCapabilities::claude(),
        "codex" => SourceCapabilities::codex(),
        "cursor" => SourceCapabilities::cursor(),
        "opencode" => SourceCapabilities::opencode(),
        "pi" => SourceCapabilities::pi(),
        "antigravity" => SourceCapabilities::antigravity(),
        _ => SourceCapabilities::generic(),
    };
    if input.agent == "antigravity" && matches!(input.source, RawSource::Sqlite(_)) {
        capabilities.cache_write_tokens = true;
    }
    let store = MemoryTurnRowStore::new(&input.agent, &input.session_id);
    let turn_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        None,
    );
    CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone()),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            kind: SourceKind::from(&input.source),
            capabilities,
        }),
        turn_rows,
    )
}

fn generate_antigravity_brain(target_bytes: usize) -> String {
    let mut jsonl = String::with_capacity(target_bytes + 256);
    let mut index = 0_u64;
    while jsonl.len() < target_bytes {
        let kind = if index.is_multiple_of(3) {
            "USER_INPUT"
        } else {
            "PLANNER_RESPONSE"
        };
        jsonl.push_str(&format!(
            "{{\"type\":\"{kind}\",\"created_at\":\"2026-01-01T00:00:00Z\",\"content\":\"Synthetic Antigravity step {index}.\",\"model\":\"MODEL_PLACEHOLDER_M35\",\"usage\":{{\"input_tokens\":13,\"output_tokens\":5}}}}\n"
        ));
        index += 1;
    }
    jsonl
}

fn generate_antigravity_cascade(steps: usize) -> String {
    let mut document = String::from(
        "{\"source\":\"antigravity_api\",\"model\":\"MODEL_PLACEHOLDER_M35\",\"steps\":{\"steps\":[",
    );
    for index in 0..steps {
        if index > 0 {
            document.push(',');
        }
        document.push_str(&format!(
            "{{\"type\":\"CORTEX_STEP_TYPE_PLANNER_RESPONSE\",\"content\":\"Synthetic cascade step {index}.\",\"metadata\":{{\"createdAt\":\"2026-01-01T00:00:00Z\"}},\"usage\":{{\"input_tokens\":13,\"output_tokens\":5}}}}"
        ));
    }
    document.push_str("]}}");
    document
}

fn protobuf_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn protobuf_field_varint(field: u64, value: u64, out: &mut Vec<u8>) {
    protobuf_varint(field << 3, out);
    protobuf_varint(value, out);
}

fn protobuf_field_bytes(field: u64, value: &[u8], out: &mut Vec<u8>) {
    protobuf_varint((field << 3) | 2, out);
    protobuf_varint(value.len() as u64, out);
    out.extend_from_slice(value);
}

fn antigravity_usage(identity: &str, input: u64, output: u64) -> Vec<u8> {
    let thinking = output / 2;
    let response = output - thinking;
    let mut usage = Vec::with_capacity(identity.len() + 48);
    protobuf_field_varint(1, 777, &mut usage);
    protobuf_field_varint(2, input, &mut usage);
    protobuf_field_varint(3, output, &mut usage);
    protobuf_field_varint(4, 7, &mut usage);
    protobuf_field_varint(5, 31, &mut usage);
    protobuf_field_varint(9, thinking, &mut usage);
    protobuf_field_varint(10, response, &mut usage);
    protobuf_field_bytes(11, identity.as_bytes(), &mut usage);
    usage
}

fn antigravity_retry(usage: &[u8]) -> Vec<u8> {
    let mut retry = Vec::with_capacity(usage.len() + 8);
    protobuf_field_bytes(2, usage, &mut retry);
    retry
}

fn antigravity_generation(primary: &[u8], retries: &[&[u8]], irrelevant_bytes: usize) -> Vec<u8> {
    let mut chat = Vec::new();
    protobuf_field_bytes(4, primary, &mut chat);
    for retry in retries {
        protobuf_field_bytes(17, &antigravity_retry(retry), &mut chat);
    }
    protobuf_field_bytes(19, b"gemini-3.6-flash", &mut chat);

    let before_len = irrelevant_bytes / 2;
    let after_len = irrelevant_bytes - before_len;
    let before = vec![0_u8; before_len];
    let after = vec![0_u8; after_len];
    let mut outer = Vec::with_capacity(chat.len() + irrelevant_bytes + 24);
    protobuf_field_bytes(30, &before, &mut outer);
    protobuf_field_bytes(7, &chat, &mut outer);
    protobuf_field_bytes(31, &after, &mut outer);
    outer
}

fn antigravity_step(seconds: u64, primary: &[u8], retries: &[&[u8]]) -> Vec<u8> {
    let mut timestamp = Vec::new();
    protobuf_field_varint(1, seconds, &mut timestamp);
    protobuf_field_varint(2, 500_000_000, &mut timestamp);

    let mut step = Vec::new();
    protobuf_field_bytes(1, &timestamp, &mut step);
    protobuf_field_bytes(9, primary, &mut step);
    for retry in retries {
        protobuf_field_bytes(28, &antigravity_retry(retry), &mut step);
    }
    step
}

struct AntigravityNativeLayout {
    _directory: TempDir,
    input: SessionInput,
    row_visits: u64,
    loaded_blob_bytes: u64,
}

fn write_antigravity_native_layout(
    generations: usize,
    sibling_transcript: bool,
    padded_generation_bytes: usize,
) -> AntigravityNativeLayout {
    let directory = TempDir::new().expect("tempdir");
    let session_id = format!("native-{generations}-{padded_generation_bytes}");
    let root = directory.path().join("antigravity-cli");
    let conversations = root.join("conversations");
    let transcript_dir = root
        .join("brain")
        .join(&session_id)
        .join(".system_generated")
        .join("logs");
    std::fs::create_dir_all(&conversations).expect("create synthetic conversations");
    std::fs::create_dir_all(&transcript_dir).expect("create synthetic brain logs");
    let transcript = concat!(
        r#"{"type":"USER_INPUT","created_at":"2026-01-01T00:00:00Z","content":"Synthetic native request."}"#,
        "\n",
        r#"{"type":"PLANNER_RESPONSE","created_at":"2026-01-01T00:00:01Z","content":"Synthetic native response.","tool_calls":[{"name":"read_file"}]}"#,
        "\n"
    );
    if sibling_transcript {
        std::fs::write(transcript_dir.join("transcript.jsonl"), transcript)
            .expect("write synthetic sibling transcript");
    }

    let db_path = conversations.join(format!("{session_id}.db"));
    let mut connection = Connection::open(&db_path).expect("create synthetic Antigravity DB");
    connection
        .execute_batch(
            "PRAGMA journal_mode = OFF;
             PRAGMA synchronous = OFF;
             CREATE TABLE trajectory_meta (trajectory_id TEXT PRIMARY KEY, cascade_id TEXT, trajectory_type INTEGER, source INTEGER);
             CREATE TABLE steps (idx INTEGER PRIMARY KEY, step_type INTEGER NOT NULL DEFAULT 0, status INTEGER NOT NULL DEFAULT 0, has_subtrajectory NUMERIC NOT NULL DEFAULT false, metadata BLOB, error_details BLOB, permissions BLOB, task_details BLOB, render_info BLOB, step_payload BLOB, step_format INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB, size INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE executor_metadata (idx INTEGER PRIMARY KEY, data BLOB);
             CREATE TABLE parent_references (idx INTEGER PRIMARY KEY, data BLOB);
             CREATE TABLE trajectory_metadata_blob (id TEXT PRIMARY KEY, data BLOB);
             CREATE TABLE battle_mode_infos (idx INTEGER PRIMARY KEY, data BLOB);",
        )
        .expect("create researched Antigravity schema");

    let transaction = connection.transaction().expect("start synthetic DB setup");
    let mut generation_bytes = 0_u64;
    let mut step_bytes = 0_u64;
    {
        let mut insert_generation = transaction
            .prepare("INSERT INTO gen_metadata(idx, data, size) VALUES (?1, ?2, ?3)")
            .expect("prepare generation insert");
        let mut insert_step = transaction
            .prepare("INSERT INTO steps(idx, metadata) VALUES (?1, ?2)")
            .expect("prepare step insert");
        for index in 0..generations {
            let primary = antigravity_usage(&format!("generation-{index}"), 101, 13);
            let failed = antigravity_usage(&format!("failed-retry-{index}"), 17, 0);
            let padding = usize::from(index == 0) * padded_generation_bytes;
            let generation =
                antigravity_generation(&primary, &[primary.as_slice(), failed.as_slice()], padding);
            generation_bytes += generation.len() as u64;
            insert_generation
                .execute(params![index as i64, generation, generation.len() as i64])
                .expect("insert synthetic generation");

            let (step_primary, step_retries) = if index.is_multiple_of(10) {
                (
                    antigravity_usage(&format!("background-{index}"), 23, 5),
                    Vec::new(),
                )
            } else {
                (primary, vec![failed])
            };
            let retry_slices: Vec<&[u8]> = step_retries.iter().map(Vec::as_slice).collect();
            let step = antigravity_step(1_767_225_600 + index as u64, &step_primary, &retry_slices);
            step_bytes += step.len() as u64;
            insert_step
                .execute(params![index as i64, step])
                .expect("insert synthetic step");
        }
    }
    transaction.commit().expect("commit synthetic DB setup");
    drop(connection);

    AntigravityNativeLayout {
        _directory: directory,
        input: SessionInput {
            agent: "antigravity".to_owned(),
            session_id,
            source: RawSource::Sqlite(db_path),
        },
        // The adapter scans generation rows twice and step rows once.
        row_visits: generations as u64 * 3,
        loaded_blob_bytes: generation_bytes * 2 + step_bytes,
    }
}

fn generate_pi_session(target_bytes: usize) -> String {
    let mut jsonl = String::with_capacity(target_bytes + 256);
    jsonl.push_str("{\"type\":\"session\",\"version\":3,\"timestamp\":\"2026-01-01T00:00:00Z\"}\n");
    let mut index = 0_u64;
    while jsonl.len() < target_bytes {
        let parent = index
            .checked_sub(1)
            .map(|value| format!(",\"parentId\":\"m{value}\""));
        jsonl.push_str(&format!(
            "{{\"type\":\"message\",\"id\":\"m{index}\"{},\"timestamp\":{},\"message\":{{\"role\":\"assistant\",\"provider\":\"synthetic-provider-{index}\",\"model\":\"synthetic-model-{index}\",\"usage\":{{\"input\":13,\"output\":5,\"cacheRead\":3,\"cacheWrite\":2}},\"content\":[]}}}}\n",
            parent.as_deref().unwrap_or(""),
            index.saturating_mul(1_000)
        ));
        index += 1;
    }
    jsonl
}

fn write_session(directory: &TempDir, session: &GeneratedSession) -> PathBuf {
    let path = directory
        .path()
        .join(format!("{}.jsonl", session.session_id));
    std::fs::write(&path, session.jsonl.as_bytes()).expect("write synthetic session");
    path
}

fn claim_for_path(path: &Path) -> SourceClaim {
    let file = std::fs::File::open(path).expect("open source for claim");
    let stat = SourceStat::from_open_std_file(&file).expect("stat source for claim");
    let bytes = std::fs::read(path).expect("read source for claim");
    SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
        stat,
        head_hash: Some(head_hash_of(&bytes)),
    })
}

fn report_context(ready_sessions: u64) -> ReportContext {
    let mut coverage = CoverageCounts::default();
    coverage.observe(CoverageBucket::Ready, ready_sessions);
    ReportContext {
        environment_key: "synthetic".to_owned(),
        window: ReportWindow {
            start_epoch: 1_770_000_000,
            end_epoch: 1_772_600_000,
        },
        computed_at_epoch: 1_772_600_000,
        parser_revision: PARSER_REVISION,
        analyzer_revision: ANALYZER_REVISION,
        evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
        coverage,
    }
}

/// Framing throughput: many small lines, and single near-bound / over-bound lines.
fn framing(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("framing");
    group.sample_size(20);

    let many_lines = generate_session_of_bytes(101, 0, 6 * MIB);
    group.throughput(Throughput::Bytes(many_lines.jsonl.len() as u64));
    group.bench_function("many_small_lines_6MiB", |bencher| {
        bencher.iter(|| {
            let mut reader = BoundedJsonlReader::new(Cursor::new(many_lines.jsonl.as_bytes()));
            let mut records = 0_u64;
            while let Some(record) = reader.next_record(&|| false) {
                black_box(&record);
                records += 1;
            }
            records
        });
    });

    let mut near_spec = SessionSpec::tier_s(103, 0, 24);
    near_spec.oversized_at = Some(12);
    near_spec.oversized_bytes = MAX_RECORD_BYTES - 64 * 1024;
    let near_max = generate_session(&near_spec);
    group.throughput(Throughput::Bytes(near_max.jsonl.len() as u64));
    group.bench_function("one_near_8MiB_line", |bencher| {
        bencher.iter(|| {
            let mut reader = BoundedJsonlReader::new(Cursor::new(near_max.jsonl.as_bytes()));
            let mut records = 0_u64;
            while let Some(record) = reader.next_record(&|| false) {
                black_box(&record);
                records += 1;
            }
            records
        });
    });

    let mut over_spec = SessionSpec::tier_s(107, 0, 24);
    over_spec.oversized_at = Some(12);
    over_spec.oversized_bytes = MAX_RECORD_BYTES + 64 * 1024;
    let over_max = generate_session(&over_spec);
    group.throughput(Throughput::Bytes(over_max.jsonl.len() as u64));
    group.bench_function("one_oversized_line_skipped", |bencher| {
        bencher.iter(|| {
            let mut reader = BoundedJsonlReader::new(Cursor::new(over_max.jsonl.as_bytes()));
            let mut records = 0_u64;
            while let Some(record) = reader.next_record(&|| false) {
                black_box(&record);
                records += 1;
            }
            records
        });
    });

    group.finish();
}

/// The append-only question: what a full reprocess costs as the file grows.
/// Runs the claimed full-validation path (pin → stream through the composite
/// sink → full recheck), exactly what the worker pays on every source change.
fn full_reparse(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("full_reparse");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(12));

    let directory = TempDir::new().expect("tempdir");
    for &size in &[MIB, 10 * MIB, 50 * MIB] {
        let session = generate_session_of_bytes(109, size / MIB, size);
        let path = write_session(&directory, &session);
        let input = file_input(&session.session_id, &path);
        let claim = claim_for_path(&path);
        group.throughput(Throughput::Bytes(session.jsonl.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("claimed_absent_guarantee", format!("{}MiB", size / MIB)),
            &size,
            |bencher, _| {
                bencher.iter(|| {
                    let mut composite = composite_for(&input);
                    let outcome = ClaudeAdapter
                        .visit_claimed(
                            &input,
                            &claim,
                            AppendOnlyGuarantee::Absent,
                            &|| false,
                            &mut composite,
                        )
                        .expect("synthetic source must stream");
                    assert!(matches!(outcome, VisitOutcome::AcceptedFull));
                    composite.observe_source_outcome(outcome);
                    black_box(composite.evidence())
                });
            },
        );
    }

    group.finish();
}

/// Antigravity brain JSONL claimed-file cost and nested cascade visitor cost.
fn antigravity(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("antigravity_pipeline");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(12));
    let directory = TempDir::new().expect("tempdir");

    for &size in &[MIB, 10 * MIB, 50 * MIB] {
        let content = generate_antigravity_brain(size);
        let path = directory
            .path()
            .join(format!("brain-{}MiB.jsonl", size / MIB));
        std::fs::write(&path, content.as_bytes()).expect("write Antigravity brain source");
        let input = file_input_for("antigravity", "synthetic-antigravity-brain", &path);
        let claim = claim_for_path(&path);
        group.throughput(Throughput::Bytes(content.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("brain_claimed", format!("{}MiB", size / MIB)),
            &size,
            |bencher, _| {
                bencher.iter(|| {
                    let mut composite = composite_for(&input);
                    let outcome = adapter_for("antigravity")
                        .visit_claimed(
                            &input,
                            &claim,
                            AppendOnlyGuarantee::Absent,
                            &|| false,
                            &mut composite,
                        )
                        .expect("Antigravity brain source must stream");
                    assert_eq!(outcome, VisitOutcome::AcceptedFull);
                    composite.observe_source_outcome(outcome);
                    black_box(composite.evidence())
                });
            },
        );
    }

    let cascade = generate_antigravity_cascade(8_000);
    let cascade_path = directory.path().join("cascade.json");
    std::fs::write(&cascade_path, cascade.as_bytes()).expect("write Antigravity cascade source");
    let input = file_input_for(
        "antigravity",
        "synthetic-antigravity-cascade",
        &cascade_path,
    );
    let claim = claim_for_path(&cascade_path);
    group.throughput(Throughput::Bytes(cascade.len() as u64));
    group.bench_function("cascade_claimed_nested_steps", |bencher| {
        bencher.iter(|| {
            let mut composite = composite_for(&input);
            let outcome = adapter_for("antigravity")
                .visit_claimed(
                    &input,
                    &claim,
                    AppendOnlyGuarantee::Absent,
                    &|| false,
                    &mut composite,
                )
                .expect("Antigravity cascade source must stream");
            assert_eq!(outcome, VisitOutcome::AcceptedFull);
            composite.observe_source_outcome(outcome);
            black_box(composite.evidence())
        });
    });

    group.finish();
}

/// Native Antigravity SQLite snapshots with descriptor-backed generation and
/// step metadata. Synthetic layout creation and insertion stay outside timing.
fn antigravity_native_db(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("antigravity_native_db");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(12));

    let paired_layouts: Vec<AntigravityNativeLayout> = [100_usize, 1_000, 10_000]
        .into_iter()
        .map(|generations| write_antigravity_native_layout(generations, true, 0))
        .collect();
    for layout in &paired_layouts {
        let generations = layout.row_visits / 3;
        group.throughput(Throughput::Elements(layout.row_visits));
        group.bench_with_input(
            BenchmarkId::new("db_plus_sibling_transcript", generations),
            layout,
            |bencher, layout| {
                bencher.iter(|| {
                    let mut composite = composite_for(&layout.input);
                    let outcome = adapter_for("antigravity")
                        .visit(&layout.input, &mut composite)
                        .expect("synthetic native Antigravity layout must stream");
                    composite.observe_source_outcome(outcome);
                    black_box(composite.evidence())
                });
            },
        );
    }

    let db_only = write_antigravity_native_layout(1_000, false, 0);
    group.throughput(Throughput::Elements(db_only.row_visits));
    group.bench_function("db_only_1000_generations", |bencher| {
        bencher.iter(|| {
            let mut composite = composite_for(&db_only.input);
            let outcome = adapter_for("antigravity")
                .visit(&db_only.input, &mut composite)
                .expect("synthetic Antigravity DB must stream");
            composite.observe_source_outcome(outcome);
            black_box(composite.evidence())
        });
    });

    let padded = write_antigravity_native_layout(1, true, 280 * KIB);
    group.throughput(Throughput::Bytes(padded.loaded_blob_bytes));
    group.bench_function("generation_with_280KiB_irrelevant_fields", |bencher| {
        bencher.iter(|| {
            let mut composite = composite_for(&padded.input);
            let outcome = adapter_for("antigravity")
                .visit(&padded.input, &mut composite)
                .expect("padded Antigravity generation must stream");
            composite.observe_source_outcome(outcome);
            black_box(composite.evidence())
        });
    });

    group.finish();
}

/// Pi claimed-file cost with a long identity chain and saturated provider hints.
fn pi(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("pi_pipeline");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(12));
    let directory = TempDir::new().expect("tempdir");

    for &size in &[MIB, 10 * MIB, 50 * MIB] {
        let content = generate_pi_session(size);
        let path = directory.path().join(format!("pi-{}MiB.jsonl", size / MIB));
        std::fs::write(&path, content.as_bytes()).expect("write Pi source");
        let input = file_input_for("pi", "synthetic-pi", &path);
        let claim = claim_for_path(&path);
        group.throughput(Throughput::Bytes(content.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("claimed", format!("{}MiB", size / MIB)),
            &size,
            |bencher, _| {
                bencher.iter(|| {
                    let mut composite = composite_for(&input);
                    let outcome = adapter_for("pi")
                        .visit_claimed(
                            &input,
                            &claim,
                            AppendOnlyGuarantee::Absent,
                            &|| false,
                            &mut composite,
                        )
                        .expect("Pi source must stream");
                    assert_eq!(outcome, VisitOutcome::AcceptedFull);
                    assert_eq!(
                        composite.summary().unwrap().provider_hints.len(),
                        antiburn_local::analysis::MAX_PROVIDER_HINTS
                    );
                    composite.observe_source_outcome(outcome);
                    black_box(composite.evidence())
                });
            },
        );
    }
    group.finish();
}

/// Per-stage attribution over one 10 MiB in-memory source: framing only,
/// + parse/normalize, + metrics, + metrics and evidence together.
fn stage_split(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("stage_split_10MiB");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(12));

    let session = generate_session_of_bytes(113, 0, 10 * MIB);
    let input = jsonl_input(&session);
    group.throughput(Throughput::Bytes(session.jsonl.len() as u64));

    group.bench_function("framing_only", |bencher| {
        bencher.iter(|| {
            let mut reader = BoundedJsonlReader::new(Cursor::new(session.jsonl.as_bytes()));
            let mut records = 0_u64;
            while let Some(record) = reader.next_record(&|| false) {
                black_box(&record);
                records += 1;
            }
            records
        });
    });

    group.bench_function("normalize_noop_sink", |bencher| {
        bencher.iter(|| {
            let mut sink = NoopSink;
            adapter_for("claude")
                .visit(&input, &mut sink)
                .expect("synthetic source must stream")
        });
    });

    group.bench_function("normalize_plus_metrics", |bencher| {
        bencher.iter(|| {
            let mut sink =
                SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
            adapter_for("claude")
                .visit(&input, &mut sink)
                .expect("synthetic source must stream");
            black_box((sink.observed_turns(), sink.retained_bytes()))
        });
    });

    group.bench_function("normalize_plus_metrics_and_evidence", |bencher| {
        bencher.iter(|| {
            let mut composite = composite_for(&input);
            let outcome = adapter_for("claude")
                .visit(&input, &mut composite)
                .expect("synthetic source must stream");
            composite.observe_source_outcome(outcome);
            black_box(composite.evidence())
        });
    });

    let normalized = normalize_source(&input).expect("synthetic source must normalize");
    group.bench_function("metrics_accumulator_only", |bencher| {
        bencher.iter_batched(
            || {
                (
                    SessionMetricsAccumulator::new("claude", "isolated"),
                    normalized.events.clone(),
                )
            },
            |(mut sink, events)| {
                for event in events {
                    sink.record(NormalizedRecord::MetricsEvent(Box::new(event)));
                }
                black_box((sink.observed_turns(), sink.retained_bytes()))
            },
            BatchSize::LargeInput,
        );
    });

    let mut disordered = normalized.events.clone();
    let event_count = disordered.len();
    for (index, event) in disordered.iter_mut().enumerate() {
        event.ts_ms = Some(
            i64::try_from(event_count.saturating_sub(index))
                .expect("synthetic event count fits")
                .saturating_mul(600_000),
        );
    }
    group.bench_function("metrics_accumulator_fully_disordered", |bencher| {
        bencher.iter_batched(
            || {
                (
                    SessionMetricsAccumulator::new("claude", "disordered"),
                    disordered.clone(),
                )
            },
            |(mut sink, events)| {
                for event in events {
                    sink.record(NormalizedRecord::MetricsEvent(Box::new(event)));
                }
                black_box((sink.observed_turns(), sink.retained_bytes()))
            },
            BatchSize::LargeInput,
        );
    });

    group.finish();
}

/// Fork-job `Inline` materialization proxy: streaming a file through the
/// pipeline vs reading the whole transcript into memory first.
fn materialization(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("materialization_10MiB");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(12));

    let directory = TempDir::new().expect("tempdir");
    let session = generate_session_of_bytes(127, 0, 10 * MIB);
    let path = write_session(&directory, &session);
    group.throughput(Throughput::Bytes(session.jsonl.len() as u64));

    let streamed = file_input(&session.session_id, &path);
    group.bench_function("stream_from_file", |bencher| {
        bencher.iter(|| {
            let mut composite = composite_for(&streamed);
            let outcome = adapter_for("claude")
                .visit(&streamed, &mut composite)
                .expect("synthetic source must stream");
            composite.observe_source_outcome(outcome);
            black_box(composite.evidence())
        });
    });

    group.bench_function("read_to_string_then_inline", |bencher| {
        bencher.iter(|| {
            let content = std::fs::read_to_string(&path).expect("materialize transcript");
            let inline = SessionInput {
                agent: "claude".to_string(),
                session_id: session.session_id.clone(),
                source: RawSource::Jsonl(content),
            };
            let mut composite = composite_for(&inline);
            let outcome = adapter_for("claude")
                .visit(&inline, &mut composite)
                .expect("synthetic source must stream");
            composite.observe_source_outcome(outcome);
            black_box(composite.evidence())
        });
    });

    group.finish();
}

/// Provider-DB-backed source through the native OpenCode message stream.
fn provider_db(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("provider_db_stream");
    group.sample_size(10);

    let directory = TempDir::new().expect("tempdir");
    for &records in &[2_000_usize, 20_000] {
        let session = generate_session(&SessionSpec::tier_s(151, records, records));
        let db_path = directory.path().join(format!("provider-{records}.db"));
        write_provider_db(&db_path, &session).expect("write synthetic provider DB");
        let db_bytes = std::fs::metadata(&db_path).expect("stat provider DB").len();
        let input = SessionInput {
            agent: "opencode".to_string(),
            session_id: session.session_id.clone(),
            source: RawSource::Sqlite(db_path),
        };
        group.throughput(Throughput::Bytes(db_bytes));
        group.bench_with_input(
            BenchmarkId::new("visit_composite", format!("{records}_rows")),
            &records,
            |bencher, _| {
                bencher.iter(|| {
                    let mut composite = composite_for(&input);
                    let outcome = adapter_for(&input.agent)
                        .visit(&input, &mut composite)
                        .expect("synthetic provider DB must be readable");
                    composite.observe_source_outcome(outcome);
                    black_box(composite.evidence())
                });
            },
        );
    }

    group.finish();
}

/// Report reduction cost against cohort size (the 30-day accumulator).
fn report_reduction(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("report_reduction");
    group.sample_size(20);

    // One evidence value per distinct session, from a representative S tier.
    let evidence_of = |session_index: usize| -> SessionEvidence {
        let session = generate_session(&SessionSpec::tier_s(131, session_index, 120));
        let input = jsonl_input(&session);
        let mut composite = composite_for(&input);
        let outcome = adapter_for("claude")
            .visit(&input, &mut composite)
            .expect("synthetic source must stream");
        composite.observe_source_outcome(outcome);
        composite.evidence().expect("clean session must publish")
    };

    for &sessions in &[10_usize, 65, 100, 500] {
        let cohort: Vec<SessionEvidence> = (0..sessions).map(evidence_of).collect();
        group.throughput(Throughput::Elements(sessions as u64));
        group.bench_with_input(
            BenchmarkId::new("observe_and_finish", sessions),
            &cohort,
            |bencher, cohort| {
                bencher.iter_batched(
                    || cohort.clone(),
                    |cohort| {
                        let mut report = EfficiencyReportAccumulator::new();
                        let count = cohort.len() as u64;
                        for evidence in cohort {
                            report.observe_session(evidence);
                        }
                        black_box(report.finish(report_context(count)))
                    },
                    BatchSize::LargeInput,
                );
            },
        );
    }

    group.finish();
}

/// Memory figures printed alongside the timing baseline (recorded in
/// BASELINE.md): framing high-water, metrics retained bytes, and a
/// serialized-evidence size proxy for the report query loop.
fn memory_probes() {
    println!("\n== memory probes (recorded in benches/BASELINE.md) ==");

    let large = generate_session_of_bytes(137, 0, 10 * MIB);
    let mut reader = BoundedJsonlReader::new(Cursor::new(large.jsonl.as_bytes()));
    while reader.next_record(&|| false).is_some() {}
    println!(
        "framing high-water, 10 MiB / {} records of small lines: {} bytes",
        large.tallies.total_records,
        reader.retained_record_bytes_high_water()
    );

    let mut near_spec = SessionSpec::tier_s(139, 0, 24);
    near_spec.oversized_at = Some(12);
    near_spec.oversized_bytes = MAX_RECORD_BYTES - 64 * 1024;
    let near_max = generate_session(&near_spec);
    let mut reader = BoundedJsonlReader::new(Cursor::new(near_max.jsonl.as_bytes()));
    while reader.next_record(&|| false).is_some() {}
    println!(
        "framing high-water, one near-8 MiB line: {} bytes (bound {})",
        reader.retained_record_bytes_high_water(),
        MAX_RECORD_BYTES
    );

    let input = jsonl_input(&large);
    let mut metrics = SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
    adapter_for("claude")
        .visit(&input, &mut metrics)
        .expect("synthetic source must stream");
    println!(
        "metrics accumulator, 10 MiB source: {} observed turns, {} retained bytes",
        metrics.observed_turns(),
        metrics.retained_bytes()
    );

    let mut composite = composite_for(&input);
    let outcome = adapter_for("claude")
        .visit(&input, &mut composite)
        .expect("synthetic source must stream");
    composite.observe_source_outcome(outcome);
    let evidence = composite.evidence().expect("clean session must publish");
    let serialized = serde_json::to_string(&evidence).expect("evidence serializes");
    println!(
        "serialized evidence for the 10 MiB session (report query row proxy): {} bytes",
        serialized.len()
    );

    let brain = generate_antigravity_brain(10 * MIB);
    let mut reader = BoundedJsonlReader::new(Cursor::new(brain.as_bytes()));
    while reader.next_record(&|| false).is_some() {}
    let framing_high_water = reader.retained_record_bytes_high_water();
    drop(reader);
    let input = SessionInput {
        agent: "antigravity".to_owned(),
        session_id: "synthetic-antigravity-memory".to_owned(),
        source: RawSource::Jsonl(brain),
    };
    let mut metrics = SessionMetricsAccumulator::new(&input.agent, &input.session_id);
    adapter_for("antigravity")
        .visit(&input, &mut metrics)
        .expect("Antigravity brain source must stream");
    println!(
        "Antigravity brain, 10 MiB: {} bytes framing high-water, {} retained metrics bytes",
        framing_high_water,
        metrics.retained_bytes()
    );
}

/// Active-writer figure: how often the full-reprocess claim is rejected with
/// `SourceChanged` while a synthetic writer keeps appending, per append
/// interval. Printed as a rate; recorded in BASELINE.md.
fn active_writer_rates() {
    println!("\n== active-writer SourceChanged rates (recorded in benches/BASELINE.md) ==");
    const ATTEMPTS: usize = 15;

    let directory = TempDir::new().expect("tempdir");
    let session = generate_session_of_bytes(149, 0, 2 * MIB);
    let path = write_session(&directory, &session);
    let input = file_input(&session.session_id, &path);

    let run_attempts = |writer_interval: Option<Duration>| -> (usize, usize) {
        let mut rejected = 0;
        let mut accepted = 0;
        for _ in 0..ATTEMPTS {
            let claim = claim_for_path(&path);
            let stop = Arc::new(AtomicBool::new(false));
            let writer = writer_interval.map(|interval| {
                let stop = Arc::clone(&stop);
                let path = path.clone();
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(interval);
                        let mut file = std::fs::OpenOptions::new()
                            .append(true)
                            .open(&path)
                            .expect("open source for append");
                        file.write_all(
                            b"{\"type\":\"assistant\",\"timestamp\":1770099999,\"message\":{\"id\":\"msg-writer\",\"role\":\"assistant\",\"model\":\"claude-3-5-haiku-20241022\",\"usage\":{\"input_tokens\":2,\"output_tokens\":3},\"content\":[{\"type\":\"text\",\"text\":\"Appended synthetic turn.\"}]}}\n",
                        )
                        .expect("append to source");
                    }
                })
            });
            let mut sink = NoopSink;
            let outcome = ClaudeAdapter
                .visit_claimed(
                    &input,
                    &claim,
                    AppendOnlyGuarantee::Absent,
                    &|| false,
                    &mut sink,
                )
                .expect("synthetic source must stream");
            stop.store(true, Ordering::Relaxed);
            if let Some(writer) = writer {
                writer.join().expect("writer thread joins");
            }
            match outcome {
                VisitOutcome::SourceChanged(_) => rejected += 1,
                VisitOutcome::AcceptedFull => accepted += 1,
                other => panic!("unexpected outcome: {other:?}"),
            }
        }
        (rejected, accepted)
    };

    for interval_ms in [2_u64, 20, 200] {
        let (rejected, accepted) = run_attempts(Some(Duration::from_millis(interval_ms)));
        println!(
            "append every {interval_ms} ms: {rejected}/{} rejected (SourceChanged), {accepted} accepted",
            rejected + accepted
        );
    }
    let (rejected, accepted) = run_attempts(None);
    println!(
        "quiescent control: {rejected}/{} rejected, {accepted} accepted",
        rejected + accepted
    );
}

fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    framing(&mut criterion);
    full_reparse(&mut criterion);
    antigravity(&mut criterion);
    antigravity_native_db(&mut criterion);
    pi(&mut criterion);
    stage_split(&mut criterion);
    materialization(&mut criterion);
    provider_db(&mut criterion);
    report_reduction(&mut criterion);
    criterion.final_summary();
    memory_probes();
    active_writer_rates();
}
