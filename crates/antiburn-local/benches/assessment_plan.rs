use std::collections::BTreeMap;
use std::hint::black_box;

use antiburn_local::analysis::SourceFormat;
use antiburn_local::analysis::ignored_instructions::{
    AssessmentInput, ContentAction, ContentEventReference, InstructionProvenance, InstructionScope,
    SessionContentEvidence, build_assessment_plan, snapshot_from_text,
};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};

fn fixture(
    actions: usize,
    rules: usize,
    utf8_percent: usize,
    branches: usize,
    text_bytes: usize,
) -> AssessmentInput {
    let rule_text = (0..rules)
        .map(|index| format!("- Follow synthetic rule number {index} exactly."))
        .collect::<Vec<_>>()
        .join("\n");
    let instruction = snapshot_from_text(
        "AGENTS.md",
        rule_text,
        InstructionProvenance::RecordedInjection,
        InstructionScope::Project,
    )
    .expect("synthetic instruction parses");
    let text_unit = if utf8_percent == 100 {
        "café naïve résumé 東京"
    } else if utf8_percent == 50 {
        "café synthetic action"
    } else {
        "synthetic action with ASCII text"
    };
    let text = if text_bytes == 0 {
        text_unit.to_owned()
    } else {
        let repeated = text_unit.repeat(text_bytes / text_unit.len() + 1);
        let mut end = text_bytes.min(repeated.len());
        while !repeated.is_char_boundary(end) {
            end -= 1;
        }
        repeated[..end].to_owned()
    };
    let actions = (0..actions)
        .map(|index| {
            let id = format!("action-{index}");
            ContentAction {
                reference: ContentEventReference {
                    id: id.clone(),
                    source_key_digest: "source".to_owned(),
                    thread_digest: format!("branch-{}", index % branches),
                    turn_index: index as u64,
                    native_record_id: Some(id),
                    part_index: 0,
                    stable: true,
                },
                timestamp_ms: Some(index as i64),
                turn_role: "assistant".to_owned(),
                turn_scope: "main".to_owned(),
                authority: "agent".to_owned(),
                kind: "assistant_text".to_owned(),
                text: text.clone(),
                tool_name: None,
                tool_call_id: None,
                normalized_fields: None,
                metadata: Default::default(),
                truncated: false,
                context_only: false,
            }
        })
        .collect();
    AssessmentInput {
        content: SessionContentEvidence {
            session_identity_digest: "synthetic-session".to_owned(),
            source_format: SourceFormat::ClaudeJsonl,
            publication_fence: 1,
            selected_input_digest: "synthetic-input".to_owned(),
            actions,
            instructions: vec![instruction],
            complete: true,
            limitations: Vec::new(),
            excluded_thinking_parts: 0,
            field_availability: Vec::new(),
        },
        prior_history_complete: true,
        activity_after_ms: None,
        boundary_positions: BTreeMap::new(),
        source_generation: 1,
        source_fingerprint: None,
        incarnation: 1,
        comparison_after: None,
    }
}

fn assessment_plan(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("build_assessment_plan");
    group.sample_size(10);
    for &(name, actions, rules, utf8, branches, text_bytes) in &[
        ("small", 16, 4, 0, 1, 0),
        ("medium", 128, 16, 50, 4, 0),
        ("large", 512, 64, 100, 16, 0),
        ("actions_16", 16, 16, 0, 4, 0),
        ("actions_128", 128, 16, 0, 4, 0),
        ("actions_512", 512, 16, 0, 4, 0),
        ("rules_4", 128, 4, 0, 4, 0),
        ("rules_16", 128, 16, 0, 4, 0),
        ("rules_64", 128, 64, 0, 4, 0),
        ("utf8_0", 128, 16, 0, 4, 0),
        ("utf8_50", 128, 16, 50, 4, 0),
        ("utf8_100", 128, 16, 100, 4, 0),
        ("branches_1", 128, 16, 0, 1, 0),
        ("branches_4", 128, 16, 0, 4, 0),
        ("branches_16", 128, 16, 0, 16, 0),
        ("text_1k", 1, 1, 0, 1, 1024),
        ("text_8k", 1, 1, 0, 1, 8 * 1024),
        ("text_32k", 1, 1, 0, 1, 32 * 1024),
        ("text_128k", 1, 1, 0, 1, 128 * 1024),
        ("text_256k", 1, 1, 0, 1, 256 * 1024),
    ] {
        let input = fixture(actions, rules, utf8, branches, text_bytes);
        group.bench_with_input(
            BenchmarkId::new("synthetic", name),
            &input,
            |bencher, input| {
                bencher.iter_batched(
                    || input.clone(),
                    |input| black_box(build_assessment_plan(black_box(input))),
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

criterion_group!(benches, assessment_plan);
criterion_main!(benches);
