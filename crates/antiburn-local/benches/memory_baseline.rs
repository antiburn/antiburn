//! Peak-heap comparison: streaming a transcript vs whole-file materialization
//! (issue #224).
//!
//! A counting global allocator records the heap high-water mark for the full
//! pipeline over the same synthetic session, once streamed from a file and
//! once materialized with `read_to_string` first. This binary is separate
//! from `pipeline_baseline` so the allocator counters do not perturb the
//! timing numbers. Results are recorded in `benches/BASELINE.md`.

#[path = "../tests/support/corpus.rs"]
mod corpus;

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, MemoryTurnRowStore, NormalizedEvent, NormalizedRecord,
    RawSource, RecordSink, Role, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SessionSummary, SourceCapabilities, SourceKind, ToolCall,
    TurnRowSink, TurnRowStore, Usage, adapter_for, merge_metrics,
};
use corpus::{
    GeneratedSession, SessionSpec, generate_session, generate_session_of_bytes,
    generate_session_of_bytes_with_identity,
};
use tempfile::TempDir;

/// Wraps the system allocator and tracks live bytes and the peak.
struct CountingAllocator;

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let live = LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !new_pointer.is_null() {
            if new_size >= layout.size() {
                let grown = new_size - layout.size();
                let live = LIVE_BYTES.fetch_add(grown, Ordering::Relaxed) + grown;
                PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
            } else {
                LIVE_BYTES.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new_pointer
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Runs `work` and returns its peak heap growth over the starting live bytes.
fn measure_peak<T>(work: impl FnOnce() -> T) -> (usize, T) {
    let live_before = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(live_before, Ordering::Relaxed);
    let value = work();
    let peak = PEAK_BYTES.load(Ordering::Relaxed);
    (peak.saturating_sub(live_before), value)
}

fn composite_for(input: &SessionInput) -> CompositeSink {
    let capabilities = match input.agent.as_str() {
        "claude" => SourceCapabilities::claude(),
        "codex" => SourceCapabilities::codex(),
        "cursor" => SourceCapabilities::cursor(),
        "opencode" => SourceCapabilities::opencode(),
        "pi" => SourceCapabilities::pi(),
        "antigravity" => SourceCapabilities::antigravity(),
        _ => SourceCapabilities::generic(),
    };
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

fn run_pipeline(input: &SessionInput) -> String {
    let mut composite = composite_for(input);
    let outcome = adapter_for(&input.agent)
        .visit(input, &mut composite)
        .expect("synthetic source must stream");
    composite.observe_source_outcome(outcome);
    let evidence = composite.evidence().expect("clean session must publish");
    serde_json::to_string(&evidence).expect("evidence serializes")
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

fn pi_memory_measurement() {
    println!("\n== Pi claimed stream with long identity chain ==");
    for size_mib in [1_usize, 10, 50] {
        let directory = TempDir::new().expect("tempdir");
        let content = generate_pi_session(size_mib * MIB);
        let source_bytes = content.len();
        let path = directory.path().join("pi.jsonl");
        std::fs::write(&path, content).expect("write Pi source");
        let input = SessionInput {
            agent: "pi".to_owned(),
            session_id: "synthetic-pi-memory".to_owned(),
            source: RawSource::File(path),
        };
        let (peak, evidence) = measure_peak(|| run_pipeline(&input));
        black_box(evidence);
        println!(
            "{size_mib} MiB Pi source ({source_bytes} bytes): streaming peak {peak} bytes ({:.2}x source)",
            peak as f64 / source_bytes as f64
        );
    }
}

fn write_session(directory: &TempDir, session: &GeneratedSession) -> PathBuf {
    let path = directory
        .path()
        .join(format!("{}.jsonl", session.session_id));
    std::fs::write(&path, session.jsonl.as_bytes()).expect("write synthetic session");
    path
}

fn file_input(session_id: &str, path: &Path) -> SessionInput {
    SessionInput {
        agent: "claude".to_string(),
        session_id: session_id.to_string(),
        source: RawSource::File(path.to_path_buf()),
    }
}

const MIB: usize = 1024 * 1024;

/// Parses serialized evidence and blanks the source kind, which is the one
/// expected difference between the file path and the inline path.
fn normalized(serialized: &str) -> serde_json::Value {
    let mut value: serde_json::Value =
        serde_json::from_str(serialized).expect("evidence parses back");
    value["provenance"]["sourceKind"] = serde_json::Value::Null;
    value
}

/// Measures the 500 MiB tier and splits the peak between the accumulators.
///
/// The generator materializes the source on demand. The repository does not
/// store a large transcript-shaped blob.
fn large_session_split() {
    println!("\n== 500 MiB tier: accumulator split ==");
    let directory = TempDir::new().expect("tempdir");
    let session = generate_session_of_bytes_with_identity(211, 0, 500 * MIB, None);
    let source_bytes = session.jsonl.len();
    let path = write_session(&directory, &session);
    let session_id = session.session_id.clone();
    drop(session);

    let input = file_input(&session_id, &path);

    let live_before = LIVE_BYTES.load(Ordering::Relaxed);
    let (metrics_peak, (observed_turns, retained_bytes, allocator_live_bytes)) =
        measure_peak(|| {
            let mut metrics =
                SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
            adapter_for("claude")
                .visit(&input, &mut metrics)
                .expect("synthetic source must stream");
            let allocator_live_bytes = LIVE_BYTES
                .load(Ordering::Relaxed)
                .saturating_sub(live_before);
            (
                metrics.observed_turns(),
                metrics.retained_bytes(),
                allocator_live_bytes,
            )
        });

    let (composite_peak, evidence) = measure_peak(|| run_pipeline(&input));

    let repeated = generate_session_of_bytes_with_identity(223, 0, 500 * MIB, Some(64));
    let repeated_path = write_session(&directory, &repeated);
    let repeated_input = file_input(&repeated.session_id, &repeated_path);
    let (repeated_peak, _) = measure_peak(|| {
        let mut metrics = SessionMetricsAccumulator::new(
            repeated_input.agent.clone(),
            repeated_input.session_id.clone(),
        );
        adapter_for("claude")
            .visit(&repeated_input, &mut metrics)
            .expect("synthetic source must stream");
        metrics.retained_bytes()
    });

    println!(
        "source {source_bytes} bytes | metrics-only peak {metrics_peak} bytes ({:.2}x source), \
         retained {retained_bytes} bytes after {observed_turns} turns | allocator live \
         {allocator_live_bytes} bytes | residual {} bytes | composite peak {composite_peak} bytes \
         ({:.2}x source) | evidence adds {} bytes to the peak | serialized evidence {} bytes | \
         repeated-id peak {repeated_peak} bytes | unique-id delta {} bytes",
        metrics_peak as f64 / source_bytes as f64,
        metrics_peak.saturating_sub(allocator_live_bytes),
        composite_peak as f64 / source_bytes as f64,
        composite_peak as i64 - metrics_peak as i64,
        evidence.len(),
        metrics_peak.saturating_sub(repeated_peak)
    );
}

fn saturated_metrics(record_count: usize) -> SessionMetricsAccumulator {
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "saturated");
    for index in 0..record_count {
        let mut event = NormalizedEvent::new(Role::Assistant);
        event.ts_ms = Some(index as i64 * 1_000);
        event.model = Some(format!("model-{}", index.min(100)));
        event.thinking_mode = Some(format!("thinking-{}", index.min(100)));
        event.speed = Some(format!("speed-{}", index.min(100)));
        event.usage = Usage {
            input_tokens: 2,
            output_tokens: 3,
            cache_read_tokens: 20_000,
            cache_creation_tokens: 2,
        };
        let mut skill = ToolCall::new("Skill");
        skill.detail = Some(format!("skill-{}", index.min(300)));
        event.tools.push(skill);
        event.tools.push(ToolCall::new(format!(
            "mcp__server-{}__tool",
            index.min(200)
        )));
        event
            .tools
            .push(ToolCall::new(format!("tool-{}", index.min(300))));
        event.may_resolve_late_tool = true;
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    }
    accumulator.finish(SessionSummary::default());
    accumulator
}

fn saturation_measurement() {
    println!("\n== saturated metrics state ==");
    for records in [40_000, 400_000] {
        let live_before = LIVE_BYTES.load(Ordering::Relaxed);
        let (_, (retained, live)) = measure_peak(|| {
            let accumulator = saturated_metrics(records);
            let retained = accumulator.retained_bytes();
            let live = LIVE_BYTES
                .load(Ordering::Relaxed)
                .saturating_sub(live_before);
            (retained, live)
        });
        println!("{records} records: retained {retained} bytes | allocator live {live} bytes");
    }
}

fn session_tree_measurement() {
    println!("\n== bounded session tree ==");
    let parent = generate_session_of_bytes(227, 0, 10 * MIB);
    let parent_input = SessionInput {
        agent: "claude".to_string(),
        session_id: parent.session_id,
        source: RawSource::Jsonl(parent.jsonl),
    };
    let child_inputs = (0..20)
        .map(|index| {
            let child = generate_session(&SessionSpec::tier_s(229, index + 1, 50));
            SessionInput {
                agent: "claude".to_string(),
                session_id: child.session_id,
                source: RawSource::Jsonl(child.jsonl),
            }
        })
        .collect::<Vec<_>>();
    let live_before = LIVE_BYTES.load(Ordering::Relaxed);
    let (peak, (retained, live)) = measure_peak(|| {
        let mut parent_metrics = SessionMetricsAccumulator::new("claude", "tree-parent");
        adapter_for("claude")
            .visit(&parent_input, &mut parent_metrics)
            .expect("synthetic parent streams");
        let mut children = Vec::new();
        for input in &child_inputs {
            let mut child = SessionMetricsAccumulator::new("claude", &input.session_id);
            adapter_for("claude")
                .visit(input, &mut child)
                .expect("synthetic child streams");
            children.push(child);
        }
        black_box(merge_metrics(&parent_metrics, &children));
        let retained = parent_metrics.retained_bytes()
            + children
                .iter()
                .map(SessionMetricsAccumulator::retained_bytes)
                .sum::<usize>();
        let live = LIVE_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(live_before);
        (retained, live)
    });
    println!(
        "one 10 MiB parent plus 20 50-turn children: retained {retained} bytes | allocator live \
         {live} bytes | peak {peak} bytes"
    );
}

fn main() {
    println!("== peak heap: streaming vs whole-file materialization ==");
    println!("(peak growth over the live baseline; the source file stays on disk)");

    for &size_mib in &[1_usize, 10, 50] {
        let directory = TempDir::new().expect("tempdir");
        let session = generate_session_of_bytes(163, 0, size_mib * MIB);
        let source_bytes = session.jsonl.len();
        let path = write_session(&directory, &session);
        let session_id = session.session_id.clone();
        // Drop the in-memory copy so only the on-disk file remains.
        drop(session);

        let streamed = file_input(&session_id, &path);
        let (streaming_peak, streamed_evidence) = measure_peak(|| run_pipeline(&streamed));
        black_box(&streamed_evidence);

        let (inline_peak, inline_evidence) = measure_peak(|| {
            let content = std::fs::read_to_string(&path).expect("materialize transcript");
            let inline = SessionInput {
                agent: "claude".to_string(),
                session_id: session_id.clone(),
                source: RawSource::Jsonl(content),
            };
            run_pipeline(&inline)
        });
        assert_eq!(
            normalized(&streamed_evidence),
            normalized(&inline_evidence),
            "both paths must publish identical evidence apart from the source kind"
        );

        println!(
            "{size_mib} MiB source ({source_bytes} bytes): streaming peak {streaming_peak} bytes \
             ({:.2}x source), inline peak {inline_peak} bytes ({:.2}x source), \
             inline/streaming {:.2}x",
            streaming_peak as f64 / source_bytes as f64,
            inline_peak as f64 / source_bytes as f64,
            inline_peak as f64 / streaming_peak.max(1) as f64,
        );
    }

    large_session_split();
    saturation_measurement();
    session_tree_measurement();
    pi_memory_measurement();
}
