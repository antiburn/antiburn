use super::development::read_source;
use super::fixtures::{Case, assemble, push};
use antiburn_local::analysis::ContentKind;
use antiburn_local::checks::over_exploring::Reason;

pub(crate) fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for (name, task, purpose, repeats, positive) in [
        (
            "large-repeat",
            "Explain the parser boundary. Reuse available results.",
            "Inspect the parser.",
            3,
            true,
        ),
        (
            "fifty-repeat",
            "Explain the parser boundary. Reuse available results.",
            "Inspect the parser.",
            50,
            true,
        ),
        (
            "explicit-verification",
            "Read the parser three times to verify that the recorded outputs match.",
            "Compare the requested three outputs.",
            3,
            false,
        ),
    ] {
        let mut parts = Vec::new();
        push(&mut parts, ContentKind::UserText, task.into(), None);
        push(&mut parts, ContentKind::AssistantText, purpose.into(), None);
        for _ in 0..repeats {
            read_source(
                &mut parts,
                "/synthetic/workspace/src/parser.rs",
                &"pub fn boundary(value: usize) -> bool { value <= 10 }\n".repeat(200),
            );
        }
        push(
            &mut parts,
            ContentKind::AssistantText,
            "Investigation boundary.".into(),
            None,
        );
        push(&mut parts, ContentKind::AssistantText, "Done.".into(), None);
        let mut case = assemble(
            format!("compact-{name}"),
            "development",
            Reason::ExcessiveWithinFileReading,
            name,
            positive,
            parts,
            "complete",
        );
        case.expected_outcome = if positive { "finding" } else { "clean" }.into();
        cases.push(case);
    }
    for (name, purpose, source, positive) in [
        (
            "unrelated-garden",
            "Inspect the gardening manual.",
            "Garden manual: prepare compost, plant seeds, and water seedlings.\n",
            true,
        ),
        (
            "explicit-unrelated-garden",
            "Inspect the gardening manual.",
            "Garden manual: prepare compost, plant seeds, and water seedlings.\n",
            true,
        ),
        (
            "necessary-dependency",
            "Inspect the boundary dependency used by the parser.",
            "Parser boundary dependency: validates the inclusive endpoint for the parser.\n",
            false,
        ),
        (
            "generic-header",
            "Inspect a possible dependency.",
            "use std::collections::BTreeMap;\n",
            false,
        ),
    ] {
        let mut parts = Vec::new();
        let task = if name == "explicit-unrelated-garden" {
            "Explain the parser boundary in src/parser.rs. Do not research gardening."
        } else {
            "Explain the parser boundary in src/parser.rs."
        };
        push(&mut parts, ContentKind::UserText, task.into(), None);
        push(&mut parts, ContentKind::AssistantText, purpose.into(), None);
        read_source(
            &mut parts,
            "/synthetic/workspace/notes/reference.txt",
            &source.repeat(400),
        );
        push(
            &mut parts,
            ContentKind::AssistantText,
            "Investigation boundary.".into(),
            None,
        );
        push(&mut parts, ContentKind::AssistantText, "Done.".into(), None);
        let mut case = assemble(
            format!("compact-{name}"),
            "development",
            Reason::UnrelatedFiles,
            name,
            positive,
            parts,
            "complete",
        );
        case.expected_outcome = if positive { "finding" } else { "abstention" }.into();
        if name == "generic-header" {
            case.expected_semantic = "uncertain".into();
        }
        cases.push(case);
    }
    for case in &mut cases {
        if case.policy_admission == "admitted" && case.expected_bindings.is_empty() {
            let reads = case
                .input
                .as_ref()
                .expect("Admitted synthetic case has prepared input")
                .episodes
                .last()
                .expect("Admitted synthetic case has an investigation episode")
                .reads
                .iter()
                .map(|read| antiburn_local::checks::over_exploring::ReadBinding {
                    request_id: read.request.reference_id.clone(),
                    result_id: read
                        .result
                        .as_ref()
                        .map(|result| result.reference_id.clone()),
                    output_digest: read
                        .result
                        .as_ref()
                        .map(|result| result.recorded_output_digest.clone()),
                })
                .collect();
            case.expected_bindings
                .push(super::fixtures::ExpectedFinding {
                    reason: case.reason,
                    reads,
                });
        }
    }
    cases
}

#[test]
fn supported_compact_targets_bind_the_exact_reads() {
    use antiburn_local::analysis::jev::JevCheck;
    use antiburn_local::checks::over_exploring::{OverExploringCheck, build_jev_context};
    for case in cases().into_iter().filter(|case| !case.expected.is_empty()) {
        let plan = OverExploringCheck
            .prepare(&build_jev_context(case.input.as_ref().unwrap()).unwrap())
            .unwrap();
        assert!(
            case.expected
                .iter()
                .all(|expected| plan
                    .prepared
                    .targets
                    .values()
                    .any(|target| target.reason == expected.reason
                        && target.bindings == expected.reads)),
            "{}",
            case.id
        );
        assert!(!plan.work_items.is_empty(), "{}", case.id);
    }
}

#[test]
fn verified_tev_context_admits_large_repeat_and_explicit_verification_requests() {
    use antiburn_local::analysis::jev::capabilities::{
        CapabilityLimit, CapabilitySource, ModelCapabilities,
    };
    use antiburn_local::analysis::jev::{
        JevCheck, estimate_jev_rendered_question_tokens, pack_work_items_with_shared_context,
        validate_jev_request_with_capabilities,
    };
    use antiburn_local::checks::over_exploring::{OverExploringCheck, build_jev_context};

    let mut capabilities = ModelCapabilities::jev_default();
    capabilities.use_ollama_tev1_accounting("chat envelope", "system text");
    capabilities.total_input_tokens =
        CapabilityLimit::known(2048, CapabilitySource::ProviderMetadata);
    capabilities.runtime_context_tokens =
        CapabilityLimit::known(2048, CapabilitySource::RuntimeMetadata);
    capabilities.state_and_longest_question_tokens =
        CapabilityLimit::known(2048, CapabilitySource::ProviderMetadata);
    capabilities.rendering_reserve_tokens = 1024;

    for id in ["compact-large-repeat", "compact-explicit-verification"] {
        let case = cases()
            .into_iter()
            .find(|case| case.id == id)
            .expect("repeat case exists");
        let context = build_jev_context(case.input.as_ref().expect("case has input"))
            .expect("case context builds");
        let plan = OverExploringCheck
            .prepare_with_capabilities(&context, &capabilities)
            .expect("repeat case prepares");
        assert_eq!(plan.work_items.len(), 1, "{id}");
        let shared = plan.shared_context.as_ref().expect("task is shared");
        let packed = pack_work_items_with_shared_context(&plan.work_items, &capabilities, shared);
        assert_eq!(packed.batches.len(), 1, "{id}");
        let request = &packed.batches[0].request;
        validate_jev_request_with_capabilities(request, &capabilities).unwrap();
        assert!(
            estimate_jev_rendered_question_tokens(request, &capabilities).unwrap() <= 1024,
            "{id}"
        );
        assert_eq!(
            plan.work_items[0].window.fields["repeat"]["count"],
            case.expected_bindings[0].reads.len()
        );
        assert!(
            plan.work_items[0].window.fields["repeat"]["redundant_output_bytes"]
                .as_u64()
                .unwrap()
                >= 8192,
            "{id}"
        );
    }
}
