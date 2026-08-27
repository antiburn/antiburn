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
    EVIDENCE_SCHEMA_REVISION, EvidenceSource, MAX_RECORD_BYTES, NormalizedRecord, PARSER_REVISION,
    RawSource, RecordSink, SessionEvidence, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SessionSummary, SourceCapabilities, SourceClaim, SourceKind,
    VisitOutcome, adapter_for, normalize_source,
};
use antiburn_local::discovery::source_version::{FingerprintInputs, SourceStat, head_hash_of};
use antiburn_local::insights::{
    CoverageBucket, CoverageCounts, EfficiencyReportAccumulator, ReportContext, ReportWindow,
};
use corpus::{
    GeneratedSession, SessionSpec, generate_session, generate_session_of_bytes, write_provider_db,
};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use tempfile::TempDir;

const MIB: usize = 1024 * 1024;

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
    SessionInput {
        agent: "claude".to_string(),
        session_id: session_id.to_string(),
        source: RawSource::File(path.to_path_buf()),
    }
}

fn composite_for(input: &SessionInput) -> CompositeSink {
    CompositeSink::new(
        SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone()),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            kind: SourceKind::from(&input.source),
            capabilities: SourceCapabilities::claude(),
        }),
    )
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

/// Provider-DB-backed source: the generic schema-agnostic SQLite walk
/// (the raw-`RawSource::Sqlite` fallback path), through the composite sink.
fn provider_db(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("provider_db_walk");
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
    stage_split(&mut criterion);
    materialization(&mut criterion);
    provider_db(&mut criterion);
    report_reduction(&mut criterion);
    criterion.final_summary();
    memory_probes();
    active_writer_rates();
}
