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
use std::sync::atomic::{AtomicUsize, Ordering};

use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, RawSource, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SourceCapabilities, SourceKind, adapter_for,
};
use corpus::{GeneratedSession, generate_session_of_bytes};
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

fn run_pipeline(input: &SessionInput) -> String {
    let mut composite = composite_for(input);
    let outcome = adapter_for("claude")
        .visit(input, &mut composite)
        .expect("synthetic source must stream");
    composite.observe_source_outcome(outcome);
    let evidence = composite.evidence().expect("clean session must publish");
    serde_json::to_string(&evidence).expect("evidence serializes")
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
}
