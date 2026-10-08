use super::development::{read_source, tool};
use super::fixtures::{Case, assemble, push};
use antiburn_local::analysis::ContentKind;
use antiburn_local::checks::over_exploring::Reason;
use serde_json::json;

pub(crate) fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for (index, (name, expected, wrong)) in [
        ("transport_scheme", "https", "http"),
        ("text_encoding", "utf-8", "ascii"),
        ("file_separator", "/", "\\"),
        ("boolean_literal", "true", "yes"),
        ("request_method", "POST", "GET"),
        ("media_type", "application/json", "text/plain"),
        ("timestamp_zone", "UTC", "LOCAL"),
        ("default_locale", "en", "de"),
        ("archive_extension", ".zip", ".tar"),
        ("record_delimiter", ";", ","),
        ("checksum_name", "sha256", "md5"),
        ("newline_name", "LF", "CRLF"),
    ]
    .into_iter()
    .enumerate()
    {
        for positive in [true, false] {
            let mut parts = Vec::new();
            let path = format!("src/{name}.rs");
            let goal = if positive {
                "Correct this literal only. The function has no inputs or dependencies. Do not review the display system."
            } else {
                "Review the complete display system as well as this literal. Report each palette and label. Compare changed content and verify the report before finishing."
            };
            push(
                &mut parts,
                ContentKind::UserText,
                format!("The {name} protocol requires {expected:?}. {goal}"),
                None,
            );
            let mut source =
                format!("pub fn protocol_value() -> &'static str {{\n    {wrong:?}\n}}\n");
            // The smaller positive cases still contain many independent bodies.
            for helper in 0..[18, 24, 32, 40][index % 4] {
                source.push_str(&format!("\nfn display_label_{helper}() -> &'static str {{\n    \"palette {helper}: gray, green, and violet; toolbar legend\"\n}}\n"));
            }
            read_source(&mut parts, &path, &source);
            tool(
                &mut parts,
                "bash",
                json!({"command":format!("cargo test {name}")}).to_string(),
                format!(
                    "test protocol_literal FAILED\nexpected {expected:?}, actual {wrong:?}\nprotocol_value has no inputs\n1 failed"
                ),
            );
            push(
                &mut parts,
                ContentKind::AssistantText,
                format!(
                    "The source and regression identify the wrong literal in protocol_value. Replace {wrong:?} with {expected:?}. The display_label functions have separate bodies and do not feed this function."
                ),
                None,
            );
            push(
                &mut parts,
                ContentKind::AssistantText,
                "Target investigation starts.".into(),
                None,
            );
            for pass in 0..3 {
                if !positive && pass == 1 {
                    tool(
                        &mut parts,
                        "edit",
                        json!({"filePath":path,"oldString":wrong,"newString":expected}).to_string(),
                        "Literal edit applied successfully.".into(),
                    );
                }
                let observed = if !positive && pass > 0 {
                    source.replace(&format!("{wrong:?}"), &format!("{expected:?}"))
                } else {
                    source.clone()
                };
                read_source(&mut parts, &path, &observed);
                if !positive {
                    push(
                        &mut parts,
                        ContentKind::AssistantText,
                        format!(
                            "Display review pass {pass}: the recorded labels and palette values support the requested full report. Compare the changed protocol literal and unchanged labels."
                        ),
                        None,
                    );
                }
            }
            push(
                &mut parts,
                ContentKind::AssistantText,
                "Investigation boundary.".into(),
                None,
            );
            if positive {
                tool(
                    &mut parts,
                    "edit",
                    json!({"filePath":path,"oldString":wrong,"newString":expected}).to_string(),
                    "Literal edit applied successfully.".into(),
                );
            }
            tool(
                &mut parts,
                "bash",
                json!({"command":format!("cargo test {name}")}).to_string(),
                "protocol_literal PASSED\n1 passed".into(),
            );
            push(
                &mut parts,
                ContentKind::AssistantText,
                if positive {
                    format!(
                        "The protocol literal is now {expected:?}. The exact regression passes. No display function is called by protocol_value or changed."
                    )
                } else {
                    format!(
                        "Protocol literal verified as {expected:?}. Display review: each recorded label uses gray, green, and violet and identifies its numbered toolbar legend. The original and changed source observations support the requested report."
                    )
                },
                None,
            );
            cases.push(assemble(
                format!(
                    "development-extents-{}-{index:02}",
                    if positive { "positive" } else { "negative" }
                ),
                "development",
                Reason::ExcessiveWithinFileReading,
                if positive {
                    "repeated_independent_content"
                } else {
                    "requested_changed_source_review"
                },
                positive,
                parts,
                "complete",
            ));
        }
    }
    cases
}
