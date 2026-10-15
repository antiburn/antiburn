use super::fixtures::{Case, assemble, push};
use antiburn_local::analysis::{ContentKind, PublishedContentPart};
use antiburn_local::checks::over_exploring::Reason;
use serde_json::json;

pub(crate) fn tool(
    parts: &mut Vec<PublishedContentPart>,
    name: &str,
    input: String,
    output: String,
) {
    let start = parts.len();
    let call = format!("{name}-{start}");
    push(parts, ContentKind::ToolInput, input, Some(call.clone()));
    push(parts, ContentKind::ToolResult, output, Some(call));
    for part in &mut parts[start..] {
        part.part.tool_name = Some(name.into());
    }
}

pub(crate) fn read_source(parts: &mut Vec<PublishedContentPart>, path: &str, source: &str) {
    let lines = source.lines().count();
    let output = source
        .lines()
        .enumerate()
        .map(|(index, line)| format!("{}: {line}\n", index + 1))
        .collect::<String>();
    tool(
        parts,
        "read",
        json!({"filePath":path,"cwd":"/synthetic/workspace","offset":1,"limit":lines}).to_string(),
        format!(
            "<path>{path}</path>\n<type>file</type>\n<content>\n{output}\n(End of file - total {lines} lines)\n</content>"
        ),
    );
}

fn core(module: &str, dependency: bool, long: bool, fixed: bool) -> String {
    let relation = if fixed { "<=" } else { "<" };
    let mut text = if dependency {
        format!(
            "mod {module}_dependency;\npub fn accepts(value: usize, maximum: usize) -> bool {{\n    {module}_dependency::validate(value, maximum)\n}}\n"
        )
    } else {
        format!(
            "pub fn accepts(value: usize, maximum: usize) -> bool {{\n    value {relation} maximum\n}}\n#[test]\nfn endpoint_is_valid() {{ assert!(accepts(10, 10)); }}\n#[test]\nfn overflow_is_invalid() {{ assert!(!accepts(11, 10)); }}\n"
        )
    };
    if long {
        for index in 0..60 {
            text.push_str(&format!("fn render_panel_{index}() -> &'static str {{\n    \"Panel {index}: choose a display theme and inspect its numbered preview swatches\"\n}}\n\n"));
        }
    }
    text
}

fn dependency_source() -> String {
    let mut source =
        "pub fn validate(value: usize, maximum: usize) -> bool {\n    value < maximum\n}\n"
            .to_owned();
    for value in 0..35 {
        source.push_str(&format!(
            "#[test]\nfn value_{value}() {{ assert_eq!(validate({value}, 10), {value} <= 10); }}\n"
        ));
    }
    source
}

pub(crate) fn primary(reason: Reason, positive: bool, variant: usize) -> Case {
    let module = [
        "cache_age",
        "parser_length",
        "retry_count",
        "cookie_age",
        "label_width",
        "migration_batch",
        "upload_size",
        "redirect_depth",
        "trace_level",
        "index_capacity",
        "currency_units",
        "wake_delay",
    ][variant];
    let family = if positive {
        "resolved_diagnosis_detour"
    } else {
        [
            "requested_audit",
            "dependency_discovery",
            "risk_check",
            "hypothesis_elimination",
            "cross_cutting_change",
            "repository_discovery",
            "legitimate_reread",
            "changed_file",
            "needed_surrounding_context",
            "future_use",
            "small_file",
            "long_context",
        ][variant]
    };
    let mut parts = Vec::new();
    let diligence = if positive && reason == Reason::ExcessiveWithinFileReading {
        "Change only the comparison."
    } else if positive {
        "Change only the inclusive endpoint comparison. No audit, refactor, or alternative implementation is requested."
    } else {
        match variant {
            0 if reason == Reason::ExcessiveWithinFileReading => {
                "Read the complete file three times. Compare the results and report without editing."
            }
            0 => "Audit the relevant validation paths and supply a written report before editing.",
            1 => "Trace the imported validation dependency before selecting the correction.",
            2 => "Check boundary safety and cleanup behavior in each affected component.",
            3 => "Rule out caller conversion and dependency validation as alternative causes.",
            4 => "Update and verify every consumer of the shared boundary contract.",
            5 => "Find the implementation and callers in this unfamiliar repository.",
            6 => {
                "Review the condition and tests separately; verify both observations before editing."
            }
            7 => "Verify the source again after the recorded edit; compare the changed condition.",
            8 => "Check the surrounding validation invariants before choosing an edit.",
            9 => {
                "Produce a test design and explanation using the reads. No source edit is required."
            }
            10 => "Explain the entire small helper and compare it against the endpoint test.",
            _ => {
                "Check the full relevant source and all caller contracts, then report the findings."
            }
        }
    };
    push(
        &mut parts,
        ContentKind::UserText,
        if reason == Reason::ExcessiveWithinFileReading && (positive || variant == 0) {
            if positive {
                format!("Fix src/{module}.rs so 10 is included and 11 is rejected. {diligence}")
            } else {
                format!("Explain src/{module}.rs. {diligence}")
            }
        } else {
            format!(
                "The {module} endpoint is inclusive: accepts(10, 10) must be true and accepts(11, 10) must be false. Correct src/{module}.rs. {diligence}"
            )
        },
        None,
    );
    let dependency = !positive && reason == Reason::UnrelatedFiles;
    let long =
        reason == Reason::ExcessiveWithinFileReading && (positive || [0, 7, 11].contains(&variant));
    let source = core(module, dependency, long, false);
    read_source(&mut parts, &format!("src/{module}.rs"), &source);
    tool(
        &mut parts,
        "bash",
        json!({"command":format!("cargo test -q {module}::endpoint_is_valid")}).to_string(),
        format!(
            "running 1 test\n{module}::endpoint_is_valid --- FAILED\nthread panicked: assertion failed: accepts(10, 10)\nExpected true; observed false\nsource: src/{module}.rs:2\n1 failed; 0 passed"
        ),
    );
    push(
        &mut parts,
        ContentKind::AssistantText,
        if positive {
            format!(
                "The recorded accepts function uses <; the failing endpoint test requires <=. I will replace that operator at src/{module}.rs:2."
            )
        } else {
            format!("Inspect the recorded {family} evidence before selecting the change.")
        },
        None,
    );
    if variant == 11 {
        let log = (0..20).map(|index|format!("test matrix {index}: accepts(9, 10)=true; accepts(11, 10)=false; endpoint contract unchanged\n")).collect::<String>();
        tool(
            &mut parts,
            "bash",
            json!({"command":"cargo test boundary_matrix -- --nocapture"}).to_string(),
            log,
        );
    }
    push(
        &mut parts,
        ContentKind::AssistantText,
        "Target investigation starts.".into(),
        None,
    );
    if !positive && reason == Reason::ExcessiveWithinFileReading && variant == 0 {
        push(
            &mut parts,
            ContentKind::AssistantText,
            "Three complete reads are required.".into(),
            None,
        );
    }
    match reason {
        Reason::UnrelatedFiles => {
            if positive {
                let topics = [
                    "sourdough hydration",
                    "orchid watering",
                    "telescope optics",
                    "quilting seams",
                    "sailing knots",
                    "ceramic glaze",
                    "bird migration",
                    "beekeeping frames",
                    "guitar tuning",
                    "camping meals",
                    "garden compost",
                    "map projections",
                ];
                let topic = topics[variant];
                let text = (0..30).map(|index|format!("Workshop {index}: {topic}.\nPrepare a clean work surface, label the sample, and measure {} parts material with {} parts water.\nRecord texture and temperature, cover the sample, inspect after {} days, and discard damaged material before repeating the process.\n",index%7+2,index%5+1,index%17+1)).collect::<String>();
                read_source(&mut parts, "notes/hobby_manual.txt", &text);
            } else {
                read_source(
                    &mut parts,
                    &format!("src/{module}_dependency.rs"),
                    &dependency_source(),
                );
            }
        }
        Reason::ExcessiveFileBreadth => unreachable!("File breadth is outside this eval suite"),
        Reason::ExcessiveWithinFileReading => {
            for index in 0..if positive { 4 } else { 3 } {
                if !positive && variant == 7 && index == 1 {
                    tool(&mut parts,"edit",json!({"filePath":format!("src/{module}.rs"),"oldString":"value < maximum","newString":"value <= maximum"}).to_string(),"Edit applied successfully: value <= maximum".into());
                }
                read_source(
                    &mut parts,
                    &format!("src/{module}.rs"),
                    &core(module, false, long, !positive && variant == 7 && index > 0),
                );
                if !positive && variant != 0 {
                    push(
                        &mut parts,
                        ContentKind::AssistantText,
                        format!(
                            "Observation {index}: endpoint implementation and tests match the requested {family} review. accepts has no external inputs. The overflow case remains rejected."
                        ),
                        None,
                    );
                }
            }
        }
    }
    push(
        &mut parts,
        ContentKind::AssistantText,
        "Investigation boundary.".into(),
        None,
    );
    if positive || variant != 9 {
        let patch_path = if dependency {
            format!("src/{module}_dependency.rs")
        } else {
            format!("src/{module}.rs")
        };
        if positive || variant != 7 || reason != Reason::ExcessiveWithinFileReading {
            tool(&mut parts,"edit",json!({"filePath":patch_path,"oldString":"value < maximum","newString":"value <= maximum"}).to_string(),"Edit applied successfully; one comparison operator changed.".into());
        }
        tool(&mut parts,"bash",json!({"command":format!("cargo test -q {module}")}).to_string(),"running 2 tests\nendpoint_is_valid ... ok\noverflow_is_invalid ... ok\n2 passed; 0 failed".into());
        tool(
            &mut parts,
            "bash",
            json!({"command":"git diff --stat"}).to_string(),
            format!("{patch_path} | 2 +-\n1 file changed, 1 insertion(+), 1 deletion(-)"),
        );
    }
    let conclusion = if positive {
        format!(
            "The endpoint correction is complete: value <= maximum. The two recorded regression tests pass. Changed only src/{module}.rs."
        )
    } else {
        format!(
            "{family} result: the original source condition excludes 10 and accepts 9. The dependency must validate inclusive endpoints; caller limits 10 through 15 must stay inclusive. The source observations establish the condition, overflow test, unchanged caller contracts and absence of cleanup side effects. Test design: assert every caller endpoint and assert endpoint+1 is rejected. These observations support the requested explanation and risk report."
        )
    };
    push(&mut parts, ContentKind::AssistantText, conclusion, None);
    assemble(
        format!(
            "development-{}-{}-{variant:02}",
            serde_json::to_value(reason)
                .expect("Typed reason serializes")
                .as_str()
                .expect("Reason serializes as a string"),
            if positive { "positive" } else { "negative" }
        ),
        "development",
        reason,
        family,
        positive,
        parts,
        "complete",
    )
}

#[cfg(test)]
mod tests {
    use super::primary;
    use crate::fixtures::REASONS;
    use antiburn_local::analysis::jev::JevCheck;
    use antiburn_local::checks::over_exploring::{OverExploringCheck, build_jev_context};

    fn assert_repeated_case_admitted(positive: bool, variant: usize, expected_reads: usize) {
        let case = primary(REASONS[1], positive, variant);
        let expected_id = format!(
            "development-excessive_within_file_reading-{}-{variant:02}",
            if positive { "positive" } else { "negative" },
        );
        assert_eq!(case.id, expected_id);
        let input = case.input.as_ref().expect("Case has prepared input");
        let context = build_jev_context(input).expect("Case context builds");
        let plan = OverExploringCheck
            .prepare(&context)
            .expect("Production preparation succeeds");
        assert_eq!(
            case.policy_admission,
            "admitted",
            "{}: task_bytes={}, complete={}, episode_state={:?}, read_bytes={:?}, work_items={}, unassessed={:?}",
            case.id,
            serde_json::to_vec(&input.task_context.fields)
                .unwrap()
                .len(),
            input.complete,
            input.episodes.last().unwrap().state,
            input
                .episodes
                .last()
                .unwrap()
                .reads
                .iter()
                .map(|read| read
                    .result
                    .as_ref()
                    .map(|result| result.recorded_output_bytes))
                .collect::<Vec<_>>(),
            plan.work_items.len(),
            plan.prepared.unassessed
        );
        let target = plan
            .prepared
            .targets
            .values()
            .find(|target| target.reason == REASONS[1])
            .unwrap_or_else(|| panic!("{}: repeated read target is admitted", case.id));
        assert_eq!(target.bindings.len(), expected_reads, "{}", case.id);
        if positive {
            assert_eq!(target.bindings, case.expected[0].reads, "{}", case.id);
        }
        assert_eq!(
            case.expected_outcome,
            if positive { "finding" } else { "clean" }
        );
        assert_eq!(case.expected.len(), usize::from(positive), "{}", case.id);
        assert_eq!(case.expected_bindings, case.expected, "{}", case.id);
        let episode = input.episodes.last().expect("Target episode exists");
        let reads = &episode.reads;
        let selected = &target.read_indexes;
        assert_eq!(selected.len(), expected_reads, "{}", case.id);
        assert!(input.complete, "{}", case.id);
        let first = &reads[selected[0]];
        let first_result = first.result.as_ref().expect("Read result exists");
        assert!(first_result.recorded_output_bytes >= 8192, "{}", case.id);
        assert!(first.request.cwd.is_some(), "{}", case.id);
        assert!(!first.request.truncated, "{}", case.id);
        assert!(!first_result.truncated, "{}", case.id);
        assert!(first_result.returned_extent.is_some(), "{}", case.id);
        for pair in selected.windows(2) {
            let previous = &reads[pair[0]];
            let current = &reads[pair[1]];
            let previous_result = previous.result.as_ref().expect("Read result exists");
            let current_result = current.result.as_ref().expect("Read result exists");
            assert!(!current.request.truncated, "{}", case.id);
            assert!(!current_result.truncated, "{}", case.id);
            assert_eq!(current_result.status, first_result.status, "{}", case.id);
            assert_eq!(current_result.kind, first_result.kind, "{}", case.id);
            assert_eq!(first.request.paths, current.request.paths, "{}", case.id);
            assert_eq!(first.request.cwd, current.request.cwd, "{}", case.id);
            assert_eq!(first.request.extent, current.request.extent, "{}", case.id);
            assert_eq!(
                first.request.native_extent, current.request.native_extent,
                "{}",
                case.id
            );
            assert_eq!(
                first_result.returned_extent, current_result.returned_extent,
                "{}",
                case.id
            );
            assert_eq!(
                first_result.recorded_output_digest, current_result.recorded_output_digest,
                "{}",
                case.id
            );
            assert!(current_result.recorded_output_bytes >= 8192, "{}", case.id);
            let result_position = episode
                .events
                .iter()
                .position(|event| input.events[*event].reference.id == previous_result.reference_id)
                .expect("Previous result belongs to episode");
            let request_position = episode
                .events
                .iter()
                .position(|event| input.events[*event].reference.id == current.request.reference_id)
                .expect("Current request belongs to episode");
            assert_eq!(result_position + 1, request_position, "{}", case.id);
        }
        let redundant_bytes = selected
            .iter()
            .skip(1)
            .map(|index| {
                reads[*index]
                    .result
                    .as_ref()
                    .expect("Read result exists")
                    .recorded_output_bytes
            })
            .sum::<u64>();
        assert!(redundant_bytes >= 8192, "{}", case.id);
    }

    #[test]
    fn all_excessive_reading_positives_are_admitted_for_complete_large_repeats() {
        for variant in 0..12 {
            assert_repeated_case_admitted(true, variant, 4);
        }
    }

    #[test]
    fn excessive_reading_audit_negative_is_admitted_for_required_repeats() {
        assert_repeated_case_admitted(false, 0, 3);
    }
}
