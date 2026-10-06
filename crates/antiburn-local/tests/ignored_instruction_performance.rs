use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use antiburn_local::analysis::SourceFormat;
use antiburn_local::analysis::ignored_instructions::{
    AssessmentInput, ContentAction, ContentEventReference, IgnoredInstructionsCheck,
    InstructionProvenance, InstructionScope, SessionContentEvidence, build_jev_context,
    select_session_content, snapshot_from_text,
};
use antiburn_local::analysis::jev::{JevCheck, JevRunProgress, pack_work_items};
use serde_json::{Value, json};

#[path = "support/ignored_instruction_native_performance.rs"]
mod native_performance;

struct CountingAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !new_pointer.is_null() {
            ALLOCATED.fetch_add(new_size, Ordering::Relaxed);
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            if new_size >= layout.size() {
                let live = LIVE.fetch_add(new_size - layout.size(), Ordering::Relaxed) + new_size
                    - layout.size();
                PEAK.fetch_max(live, Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new_pointer
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Clone, Copy)]
struct Shape {
    name: &'static str,
    actions: usize,
    rules: usize,
    bytes: usize,
    branches: usize,
    utf8: bool,
    output_heavy: bool,
    long_rule: bool,
    tool_fields: bool,
}

fn fixture(shape: Shape) -> AssessmentInput {
    let rules = if shape.long_rule {
        format!(
            "Do not force push. {} Do not force push.",
            "Keep the recorded action in scope. ".repeat(256)
        )
    } else {
        (0..shape.rules)
            .map(|index| format!("- Do not force push to remote number {index}."))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let instruction = snapshot_from_text(
        "AGENTS.md",
        rules,
        InstructionProvenance::RecordedInjection,
        InstructionScope::Project,
    )
    .unwrap();
    let unit = if shape.utf8 {
        "日本語の説明。\n"
    } else {
        "Background text.\n"
    };
    let text = format!(
        "{}I requested git push --force.",
        unit.repeat(shape.bytes.div_ceil(unit.len()))
    );
    let actions = (0..shape.actions)
        .map(|index| {
            let (kind, authority, role, tool, text) = if shape.output_heavy && index % 2 == 0 {
                (
                    "tool_result",
                    "tool",
                    "tool",
                    Some("Bash"),
                    "EXCLUDED_OUTPUT\n".repeat(16 * 1024),
                )
            } else if shape.tool_fields {
                (
                    "tool_input",
                    "assistant",
                    "assistant",
                    Some("Bash"),
                    json!({"command":"git status","timeout":1,"cwd":"/workspace"}).to_string(),
                )
            } else {
                (
                    "assistant_text",
                    "assistant",
                    "assistant",
                    None,
                    text.clone(),
                )
            };
            ContentAction {
                reference: ContentEventReference {
                    id: format!("e{index}"),
                    source_key_digest: "synthetic-source".to_owned(),
                    thread_digest: format!("branch{}", index % shape.branches),
                    turn_index: index as u64,
                    native_record_id: Some(format!("e{index}")),
                    part_index: 0,
                    stable: true,
                },
                timestamp_ms: Some(index as i64),
                turn_role: role.to_owned(),
                turn_scope: "main".to_owned(),
                authority: authority.to_owned(),
                kind: kind.to_owned(),
                text,
                tool_name: tool.map(str::to_owned),
                tool_call_id: tool.map(|_| format!("call{index}")),
                normalized_fields: None,
                metadata: Default::default(),
                truncated: false,
                context_only: false,
            }
        })
        .collect();
    AssessmentInput {
        content: SessionContentEvidence {
            session_identity_digest: "synthetic-performance".to_owned(),
            source_format: SourceFormat::ClaudeJsonl,
            publication_fence: 1,
            selected_input_digest: "unselected".to_owned(),
            actions,
            instructions: vec![instruction],
            complete: true,
            limitations: Vec::new(),
            excluded_thinking_parts: 0,
            field_availability: Vec::new(),
        },
        prior_history_complete: true,
        comparison_after: None,
        boundary_positions: BTreeMap::new(),
        activity_after_ms: None,
        source_generation: 1,
        source_fingerprint: None,
        incarnation: 1,
    }
}

#[derive(Default)]
struct Sample {
    projection_us: u128,
    context_plan_us: u128,
    prepare_us: u128,
    packing_us: u128,
    reduction_us: u128,
    serialization_us: u128,
    allocation_bytes: usize,
    allocation_count: usize,
    peak_bytes: usize,
    retained_bytes: usize,
    selected_bytes: usize,
    requests: usize,
    request_bytes: usize,
    largest_request_bytes: usize,
    questions: usize,
    progress_bytes: usize,
    result_bytes: usize,
}

fn measure(input: &AssessmentInput) -> Sample {
    let baseline = LIVE.load(Ordering::Relaxed);
    let allocated_before = ALLOCATED.load(Ordering::Relaxed);
    let count_before = ALLOCATIONS.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let mut sample = Sample::default();
    {
        let started = Instant::now();
        let selected = AssessmentInput {
            content: select_session_content(
                &input.content,
                IgnoredInstructionsCheck.input_selection(),
            ),
            prior_history_complete: input.prior_history_complete,
            comparison_after: input.comparison_after.clone(),
            boundary_positions: input.boundary_positions.clone(),
            activity_after_ms: input.activity_after_ms,
            source_generation: input.source_generation,
            source_fingerprint: input.source_fingerprint.clone(),
            incarnation: input.incarnation,
        };
        sample.projection_us = started.elapsed().as_micros();
        sample.selected_bytes = selected
            .content
            .actions
            .iter()
            .map(|event| event.text.len())
            .sum();
        let started = Instant::now();
        let context = build_jev_context(&selected).unwrap();
        sample.context_plan_us = started.elapsed().as_micros();
        let started = Instant::now();
        let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
        sample.prepare_us = started.elapsed().as_micros();
        let started = Instant::now();
        let packed = pack_work_items(&plan.work_items);
        sample.packing_us = started.elapsed().as_micros();
        assert!(
            packed.skipped_item_ids.is_empty(),
            "benchmark lost work items"
        );
        for batch in &packed.batches {
            assert_eq!(
                batch.serialized_bytes,
                serde_json::to_vec(&batch.request).unwrap().len()
            );
            assert!(
                batch.request.questions.len()
                    <= antiburn_local::analysis::jev::MAX_QUESTIONS_PER_REQUEST
            );
        }
        sample.largest_request_bytes = packed
            .batches
            .iter()
            .map(|batch| batch.serialized_bytes)
            .max()
            .unwrap_or(0);
        sample.requests = packed.batches.len();
        sample.request_bytes = packed
            .batches
            .iter()
            .map(|batch| batch.serialized_bytes)
            .sum();
        sample.questions = packed
            .batches
            .iter()
            .map(|batch| batch.request.questions.len())
            .sum();
        let started = Instant::now();
        let result = IgnoredInstructionsCheck.reduce(&plan, &[], false).unwrap();
        sample.reduction_us = started.elapsed().as_micros();
        let started = Instant::now();
        let progress = JevRunProgress {
            input_revision: context.input_revision.clone(),
            ..Default::default()
        };
        sample.progress_bytes = serde_json::to_vec(&progress).unwrap().len();
        sample.result_bytes = serde_json::to_vec(&result).unwrap().len();
        sample.serialization_us = started.elapsed().as_micros();
        std::hint::black_box((&context, &plan, &packed, &result));
    }
    sample.peak_bytes = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    sample.retained_bytes = LIVE.load(Ordering::Relaxed).saturating_sub(baseline);
    sample.allocation_bytes = ALLOCATED.load(Ordering::Relaxed) - allocated_before;
    sample.allocation_count = ALLOCATIONS.load(Ordering::Relaxed) - count_before;
    sample
}

fn median(values: impl Iterator<Item = u128>) -> u128 {
    let mut values = values.collect::<Vec<_>>();
    values.sort_unstable();
    values[values.len() / 2]
}

fn profile_path(profile: &str) -> std::path::PathBuf {
    let suffix = std::env::var("JEV_PERFORMANCE_RUN")
        .map(|name| {
            assert!(
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            );
            format!("-{name}")
        })
        .unwrap_or_default();
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../.agent-artifacts/reviews/jev-release-{profile}-results{suffix}.json"
    ))
}

#[test]
fn native_selected_query_preserves_actions_and_excludes_large_outputs() {
    native_performance::verify_selected_query();
}

#[test]
#[ignore = "release-only isolated allocation and stage benchmark"]
fn release_stage_and_heap_profile() {
    if cfg!(debug_assertions) {
        panic!("use cargo test --release");
    }
    let small = Shape {
        name: "small",
        actions: 16,
        rules: 4,
        bytes: 128,
        branches: 1,
        utf8: false,
        output_heavy: false,
        long_rule: false,
        tool_fields: false,
    };
    let shapes = [
        small,
        Shape {
            name: "medium",
            actions: 128,
            rules: 16,
            branches: 4,
            ..small
        },
        Shape {
            name: "large_page",
            actions: 256,
            rules: 64,
            branches: 16,
            ..small
        },
        Shape {
            name: "many_short_rules",
            actions: 32,
            rules: 128,
            ..small
        },
        Shape {
            name: "one_long_rule",
            long_rule: true,
            rules: 1,
            ..small
        },
        Shape {
            name: "many_branches",
            branches: 16,
            actions: 128,
            ..small
        },
        Shape {
            name: "output_heavy",
            output_heavy: true,
            actions: 128,
            ..small
        },
        Shape {
            name: "tiny_structured_fields",
            tool_fields: true,
            actions: 256,
            ..small
        },
        Shape {
            name: "utf8",
            utf8: true,
            actions: 128,
            ..small
        },
        Shape {
            name: "text_1k",
            actions: 1,
            rules: 1,
            bytes: 1024,
            ..small
        },
        Shape {
            name: "text_8k",
            actions: 1,
            rules: 1,
            bytes: 8192,
            ..small
        },
        Shape {
            name: "text_32k",
            actions: 1,
            rules: 1,
            bytes: 32768,
            ..small
        },
        Shape {
            name: "text_128k",
            actions: 1,
            rules: 1,
            bytes: 131072,
            ..small
        },
        Shape {
            name: "text_256k",
            actions: 1,
            rules: 1,
            bytes: 262144,
            ..small
        },
    ];
    let mut rows = Vec::new();
    for shape in shapes {
        let input = fixture(shape);
        measure(&input);
        let samples = (0..7).map(|_| measure(&input)).collect::<Vec<_>>();
        let sample = &samples[0];
        let row = json!({"name":shape.name,"actions":shape.actions,"rules":shape.rules,"text_bytes":shape.bytes,
            "projection_us":median(samples.iter().map(|sample|sample.projection_us)),
            "context_plan_us":median(samples.iter().map(|sample|sample.context_plan_us)),
            "prepare_us":median(samples.iter().map(|sample|sample.prepare_us)),
            "packing_us":median(samples.iter().map(|sample|sample.packing_us)),
            "reduction_us":median(samples.iter().map(|sample|sample.reduction_us)),
            "serialization_us":median(samples.iter().map(|sample|sample.serialization_us)),
            "allocated_bytes":median(samples.iter().map(|sample|sample.allocation_bytes as u128)),
            "allocation_count":median(samples.iter().map(|sample|sample.allocation_count as u128)),
            "peak_live_heap_bytes":samples.iter().map(|sample|sample.peak_bytes).max().unwrap(),
            "retained_after_teardown_bytes":samples.iter().map(|sample|sample.retained_bytes).max().unwrap(),
            "selected_bytes":sample.selected_bytes,"requests":sample.requests,"request_bytes":sample.request_bytes,
            "largest_request_bytes":sample.largest_request_bytes,
            "questions":sample.questions,"progress_bytes":sample.progress_bytes,"result_bytes":sample.result_bytes});
        eprintln!("{row}");
        rows.push(row);
    }
    let report: Value = json!({"mode":"release","warmup":1,"samples":7,
        "revisions":IgnoredInstructionsCheck.revisions(),
        "boundary":"Fixture construction and excluded raw input are outside measurement. Reduction has no provider answers. Progress is empty; this does not measure durable checkpoint writes or desktop idle memory.",
        "baseline":"No comparable release allocation baseline is available; do not infer speedup from debug phase-0 samples.",
        "rows":rows});
    let output = profile_path("stage-heap");
    std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    let budgets: Value = serde_json::from_str(include_str!(
        "fixtures/ignored_instructions/evaluation_memory_budgets.json"
    ))
    .unwrap();
    for row in report["rows"].as_array().unwrap() {
        assert!(
            row["peak_live_heap_bytes"].as_u64().unwrap()
                <= budgets["transient_live_heap_bytes"].as_u64().unwrap()
        );
        assert_eq!(
            row["retained_after_teardown_bytes"],
            budgets["retained_after_teardown_bytes"]
        );
        assert!(
            row["largest_request_bytes"].as_u64().unwrap()
                <= budgets["serialized_request_bytes"].as_u64().unwrap()
        );
        assert!(
            row["result_bytes"].as_u64().unwrap()
                <= budgets["serialized_result_bytes"].as_u64().unwrap()
        );
        assert!(
            row["selected_bytes"].as_u64().unwrap()
                <= budgets["selected_page_bytes"].as_u64().unwrap()
        );
    }
}

#[test]
#[ignore = "release-only native-reader and SQLite selected-query benchmark"]
fn release_native_store_and_query_profile() {
    if cfg!(debug_assertions) {
        panic!("use cargo test --release");
    }
    let report = native_performance::run();
    let output = profile_path("native-store");
    std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
}
