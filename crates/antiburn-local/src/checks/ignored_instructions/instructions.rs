//! Instruction snapshots and structure-aware Markdown segmentation.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_INSTRUCTION_BYTES: usize = 128 * 1024;
pub const MAX_RULE_SECTION_BYTES: usize = MAX_INSTRUCTION_BYTES;
const MAX_SPLIT_LIST_ITEMS: usize = 256;
const MAX_CHUNK_BYTES: usize = 8 * 1024;
const CHUNK_OVERLAP_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionProvenance {
    RecordedInjection,
    ObservedRead,
    CurrentFileComparison,
}

impl InstructionProvenance {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RecordedInjection => "recorded_injection",
            Self::ObservedRead => "observed_read",
            Self::CurrentFileComparison => "current_file_comparison",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionScope {
    Global,
    Project,
    Nested,
    Conditional,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionRuleSection {
    pub id: String,
    pub heading: String,
    pub content_class: InstructionContentClass,
    pub text: String,
    pub start_line: u32,
    pub end_line: u32,
    pub evaluable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionContentClass {
    RequirementCandidate,
    BackgroundOrExample,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionSnapshot {
    pub id: String,
    pub digest: String,
    pub source: String,
    pub provenance: InstructionProvenance,
    pub scope: InstructionScope,
    pub text: String,
    pub sections: Vec<InstructionRuleSection>,
    pub imports: Vec<String>,
    pub limitations: Vec<String>,
}

pub fn snapshot_from_text(
    source: impl Into<String>,
    text: String,
    provenance: InstructionProvenance,
    scope: InstructionScope,
) -> Result<InstructionSnapshot, MarkdownLimit> {
    let source = source.into();
    let digest = sha256_hex(text.as_bytes());
    let id = sha256_hex(format!("{source}\0{digest}").as_bytes());
    let sections = segment_markdown(&source, &text)?;
    Ok(InstructionSnapshot {
        id,
        digest,
        source,
        provenance,
        scope,
        text,
        sections,
        imports: Vec::new(),
        limitations: Vec::new(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkdownLimit {
    FileTooLarge,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

/// Split Markdown at headings and top-level list items. Keep frontmatter,
/// nested conditions, exceptions, and examples with their rule text.
pub fn segment_markdown(
    source: &str,
    markdown: &str,
) -> Result<Vec<InstructionRuleSection>, MarkdownLimit> {
    if markdown.len() > MAX_INSTRUCTION_BYTES {
        return Err(MarkdownLimit::FileTooLarge);
    }

    let source_lines: Vec<&str> = markdown.lines().collect();
    let frontmatter_line_count = yaml_frontmatter_line_count(&source_lines).unwrap_or_default();
    let frontmatter = source_lines[..frontmatter_line_count].join("\n");
    let lines = source_lines[frontmatter_line_count..].to_vec();
    let line_offset = frontmatter_line_count as u32;
    let mut sections = Vec::new();
    let mut heading_stack: Vec<(usize, String, String)> = Vec::new();
    let mut document_context = String::new();
    let mut body = String::new();
    let mut start_line = line_offset.saturating_add(1);
    let mut in_fence = false;

    let emit = |sections: &mut Vec<InstructionRuleSection>,
                heading_stack: &[(usize, String, String)],
                body: &str,
                document_context: &str,
                start_line: u32,
                end_line: u32|
     -> Result<(), MarkdownLimit> {
        if body.trim().is_empty() {
            return Ok(());
        }
        let heading = heading_stack
            .iter()
            .map(|(_, title, _)| title.as_str())
            .collect::<Vec<_>>()
            .join(" / ");
        let content_class = if background_heading(&heading) && !contains_binding_language(body) {
            InstructionContentClass::BackgroundOrExample
        } else {
            InstructionContentClass::RequirementCandidate
        };
        let lines: Vec<&str> = body.lines().collect();
        let mut in_code = false;
        let mut items = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
                in_code = !in_code;
            } else if !in_code && top_level_list_item(line) {
                items.push(index);
            }
        }
        let ranges: Vec<(usize, usize)> = if items.len() > 1 && items.len() <= MAX_SPLIT_LIST_ITEMS
        {
            items
                .iter()
                .enumerate()
                .map(|(index, first)| {
                    (*first, items.get(index + 1).copied().unwrap_or(lines.len()))
                })
                .collect()
        } else {
            vec![(0, lines.len())]
        };
        for (first, last) in ranges {
            let prefix = if first > 0 {
                lines[..items[0]].join("\n")
            } else {
                String::new()
            };
            let rule_text = if prefix.trim().is_empty() {
                lines[first..last].join("\n")
            } else {
                format!("{prefix}\n{}", lines[first..last].join("\n"))
            };
            let text = heading_stack
                .iter()
                .map(|(_, _, context)| context.as_str())
                .filter(|context| !context.trim().is_empty())
                .chain((!frontmatter.is_empty()).then_some(frontmatter.as_str()))
                .chain((!document_context.trim().is_empty()).then_some(document_context))
                .chain(std::iter::once(rule_text.as_str()))
                .collect::<Vec<_>>()
                .join("\n");
            let item_start = start_line.saturating_add(first as u32);
            let item_end = if last == lines.len() {
                end_line
            } else {
                start_line.saturating_add(last as u32).saturating_sub(1)
            };
            let digest = sha256_hex(text.as_bytes());
            let identity = format!("{source}\0{digest}\0{heading}");
            let evaluable = text.len() <= MAX_RULE_SECTION_BYTES;
            sections.push(InstructionRuleSection {
                id: sha256_hex(identity.as_bytes()),
                heading: heading.clone(),
                content_class,
                text,
                start_line: item_start,
                end_line: item_end,
                evaluable,
            });
        }
        Ok(())
    };

    let mut skip_setext_underline = false;
    for (index, line) in lines.iter().enumerate() {
        if skip_setext_underline {
            skip_setext_underline = false;
            continue;
        }
        let line_number = line_offset.saturating_add(index as u32).saturating_add(1);
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        }
        let setext = (!in_fence)
            .then(|| lines.get(index + 1).map(|next| (trimmed, next.trim())))
            .flatten()
            .and_then(|(title, underline)| {
                let level = if !title.is_empty()
                    && !underline.is_empty()
                    && underline.chars().all(|c| c == '=')
                {
                    Some(1)
                } else if !title.is_empty()
                    && !underline.is_empty()
                    && underline.chars().all(|c| c == '-')
                {
                    Some(2)
                } else {
                    None
                }?;
                Some((level, title))
            });
        let heading = if !in_fence {
            markdown_heading(trimmed).or(setext)
        } else {
            None
        };
        if let Some((level, title)) = heading {
            if heading_stack.is_empty() {
                document_context.clone_from(&body);
            } else {
                emit(
                    &mut sections,
                    &heading_stack,
                    &body,
                    &document_context,
                    start_line,
                    line_number.saturating_sub(1),
                )?;
            }
            if let Some((_, _, context)) = heading_stack.last_mut() {
                *context = body.clone();
            }
            body.clear();
            while heading_stack
                .last()
                .is_some_and(|(parent_level, _, _)| *parent_level >= level)
            {
                heading_stack.pop();
            }
            heading_stack.push((level, title.to_owned(), String::new()));
            start_line = line_number + if setext.is_some() { 2 } else { 1 };
            if setext.is_some() {
                skip_setext_underline = true;
            }
        } else {
            if body.is_empty() && line.trim().is_empty() {
                start_line = line_number.saturating_add(1);
                continue;
            }
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(line);
        }
    }
    emit(
        &mut sections,
        &heading_stack,
        &body,
        &document_context,
        start_line,
        line_offset.saturating_add(lines.len() as u32),
    )?;
    Ok(sections)
}

fn background_heading(heading: &str) -> bool {
    heading.split(" / ").any(|part| {
        let normalized = part
            .trim_matches(|character: char| !character.is_ascii_alphanumeric())
            .to_ascii_lowercase();
        matches!(
            normalized.as_str(),
            "example" | "examples" | "background" | "context" | "reference" | "references"
        )
    })
}

fn contains_binding_language(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "must ",
        "must not ",
        "do not ",
        "never ",
        "always ",
        "required to ",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// Split long text at structural boundaries and include its source range with
/// each fragment. Every returned range is a valid UTF-8 boundary.
pub fn chunk_text_with_ranges(text: &str) -> Vec<(usize, usize)> {
    if text.len() <= MAX_CHUNK_BYTES {
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![(0, text.len())]
        };
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let limit = start.saturating_add(MAX_CHUNK_BYTES).min(text.len());
        let mut end = limit;
        if limit < text.len() {
            let floor = text.floor_char_boundary(limit);
            let window = &text[start..floor];
            let candidate = window
                .match_indices("\n\n")
                .last()
                .map(|(offset, _)| start + offset + 2)
                .or_else(|| structural_line_boundary(window).map(|offset| start + offset))
                .or_else(|| window.rfind('\n').map(|offset| start + offset + 1));
            if let Some(candidate) = candidate.filter(|candidate| *candidate > start) {
                end = candidate;
            } else {
                end = floor;
            }
        }
        if end <= start {
            end = text[start..]
                .char_indices()
                .nth(1)
                .map_or(text.len(), |(offset, _)| start + offset);
        }
        chunks.push((start, end));
        if end == text.len() {
            break;
        }
        let overlap_target = end.saturating_sub(CHUNK_OVERLAP_BYTES);
        let next = text[overlap_target..end]
            .char_indices()
            .find(|(_, character)| *character == '\n')
            .map(|(offset, _)| overlap_target + offset + 1)
            .unwrap_or_else(|| {
                text[overlap_target..end]
                    .char_indices()
                    .next()
                    .map_or(end, |(offset, _)| overlap_target + offset)
            });
        start = next.max(start + 1);
    }
    chunks
}

/// Prefer complete records when the input has a recognizable line format.
/// This is boundary detection only; it does not claim to parse the input.
fn structural_line_boundary(text: &str) -> Option<usize> {
    text.match_indices('\n')
        .filter_map(|(offset, _)| {
            let line_start = offset + 1;
            let line = text[line_start..].lines().next()?.trim_start();
            let boundary = markdown_rule_start(line)
                || line.starts_with("diff --git ")
                || line.starts_with("--- ")
                || line.starts_with("+++ ")
                || line.starts_with("@@ ")
                || json_field_start(line)
                || shell_record_start(line);
            boundary.then_some(line_start)
        })
        .next_back()
}

fn markdown_rule_start(line: &str) -> bool {
    markdown_heading(line).is_some()
        || line.starts_with("- ")
        || line.starts_with("* ")
        || line.starts_with("+ ")
        || line.split_once(". ").is_some_and(|(number, _)| {
            !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
        })
}

fn json_field_start(line: &str) -> bool {
    let line = line.trim_start_matches([',', '{', ' ']);
    line.starts_with('"') && line.contains("\":")
}

fn shell_record_start(line: &str) -> bool {
    line.starts_with("$")
        || line.starts_with("#!")
        || line.starts_with("cat <<")
        || line.starts_with("<<")
}

fn yaml_frontmatter_line_count(lines: &[&str]) -> Option<usize> {
    if lines.first()?.trim() != "---" {
        return None;
    }
    let end = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(index, _)| index)?;
    lines
        .get(1..end)?
        .iter()
        .any(|line| line.split_once(':').is_some())
        .then_some(end + 1)
}

fn top_level_list_item(line: &str) -> bool {
    let text = if let Some(rest) = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
    {
        rest
    } else if let Some((number, rest)) = line.split_once(". ") {
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        rest
    } else {
        return false;
    };
    let lower = text.to_ascii_lowercase();
    !text.is_empty()
        && ![
            "exception",
            "except",
            "unless",
            "otherwise",
            "example",
            "e.g.",
        ]
        .iter()
        .any(|word| lower.starts_with(word))
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let level = line
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if (1..=6).contains(&level) && line.as_bytes().get(level) == Some(&b' ') {
        let title = line[level..].trim();
        return (!title.is_empty()).then_some((level, title));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segmentation_keeps_nested_conditions_and_exceptions_together() {
        let markdown = "# Rules\n\n- Do not edit generated files.\n  - Exception: run the snapshot command.\n\n## Other\nUse the same API.";
        let sections = segment_markdown("AGENTS.md", markdown).unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading, "Rules");
        assert!(
            sections[0]
                .text
                .contains("Exception: run the snapshot command")
        );
        assert_eq!(sections[1].heading, "Rules / Other");
    }

    #[test]
    fn descendant_rules_keep_parent_conditions_and_source_lines() {
        let sections = segment_markdown("AGENTS.md", "# Rules\nOnly when publishing a release.\n\n## Checks\nRun the tests.\n\n### Report\nReport failures.").unwrap();
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[1].start_line, 5);
        assert_eq!(sections[2].start_line, 8);
        assert!(sections[1].text.contains("Only when publishing a release."));
        assert!(sections[2].text.contains("Only when publishing a release."));
        assert!(sections[2].text.contains("Run the tests."));
    }

    #[test]
    fn document_preamble_conditions_apply_to_top_level_and_nested_rules() {
        let sections = segment_markdown(
            "AGENTS.md",
            "Only when publishing a release.\n\n# Commands\nNever invoke `Bash`.\n\n## Reports\nDo not expose secrets.",
        )
        .unwrap();

        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading, "Commands");
        assert_eq!(sections[1].heading, "Commands / Reports");
        assert!(sections[0].text.contains("Only when publishing a release."));
        assert!(sections[1].text.contains("Only when publishing a release."));
        assert_eq!(sections[0].start_line, 4);
        assert_eq!(sections[1].start_line, 7);
    }

    #[test]
    fn document_preamble_applies_to_setext_headings_and_does_not_leak_between_siblings() {
        let sections = segment_markdown(
            "AGENTS.md",
            "Only during release work.\n\nFirst\n=====\nRun the release checks.\n\nSecond\n=====\nReview the release notes.",
        )
        .unwrap();

        assert_eq!(sections.len(), 2);
        assert!(sections[0].text.contains("Only during release work."));
        assert!(sections[1].text.contains("Only during release work."));
        assert!(!sections[0].text.contains("Review the release notes."));
        assert!(!sections[1].text.contains("Run the release checks."));
        assert_eq!(sections[0].start_line, 5);
        assert_eq!(sections[1].start_line, 9);
    }

    #[test]
    fn independent_list_rules_have_distinct_ranges_and_keep_shared_context() {
        let sections = segment_markdown(
            "AGENTS.md",
            "# Checks\nApplies to release work.\n\n- Run tests before a commit.\n  - Exception: generated files only.\n- Ask before adding a dependency.\n\n## Next\nFollow the style guide.",
        )
        .unwrap();
        assert_eq!(sections.len(), 3);
        assert!(sections[0].text.contains("Applies to release work."));
        assert!(
            sections[0]
                .text
                .contains("Exception: generated files only.")
        );
        assert!(!sections[0].text.contains("Ask before adding"));
        assert!(sections[1].text.contains("Applies to release work."));
        assert_eq!(sections[0].start_line, 4);
        assert_eq!(sections[1].start_line, 6);
        assert_ne!(sections[0].id, sections[1].id);
    }

    #[test]
    fn long_lists_keep_all_items_without_repeating_shared_context() {
        let markdown = format!(
            "# Rules\nApplies to every operation.\n{}",
            (0..300)
                .map(|index| format!("- Follow step {index}."))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let sections = segment_markdown("AGENTS.md", &markdown).unwrap();

        assert_eq!(sections.len(), 1);
        assert!(sections[0].text.contains("Follow step 0."));
        assert!(sections[0].text.contains("Follow step 299."));
        assert!(sections[0].text.contains("Applies to every operation."));
    }

    #[test]
    fn top_level_exceptions_and_code_examples_stay_with_the_rule() {
        let sections = segment_markdown(
            "AGENTS.md",
            "# Rules\n- Do not edit generated files.\n- Exception: refresh snapshots first.\n```md\n- This is only an example.\n```\n- Ask before changing dependencies.",
        )
        .unwrap();
        assert_eq!(sections.len(), 2);
        assert!(
            sections[0]
                .text
                .contains("Exception: refresh snapshots first")
        );
        assert!(sections[0].text.contains("This is only an example"));
        assert!(!sections[1].text.contains("Exception:"));
    }

    #[test]
    fn headings_inside_fenced_code_do_not_split_a_rule() {
        let sections = segment_markdown(
            "AGENTS.md",
            "# Rule\n\n```md\n## example\n```\nKeep this rule.",
        )
        .unwrap();
        assert_eq!(sections.len(), 1);
        assert!(sections[0].text.contains("## example"));
    }

    #[test]
    fn stable_ids_change_when_source_text_changes() {
        let first = segment_markdown("AGENTS.md", "# Rule\nDo this.").unwrap();
        let second = segment_markdown("AGENTS.md", "# Rule\nDo that.").unwrap();
        assert_ne!(first[0].id, second[0].id);
    }

    #[test]
    fn rule_identity_survives_line_shifts_but_keeps_source_scope() {
        let first = segment_markdown("AGENTS.md", "# Rule\nDo this.").unwrap();
        let shifted =
            segment_markdown("AGENTS.md", "# Note\nBackground.\n\n# Rule\nDo this.").unwrap();
        let other_source = segment_markdown("other/AGENTS.md", "# Rule\nDo this.").unwrap();

        assert_eq!(first[0].id, shifted[1].id);
        assert_ne!(first[0].id, other_source[0].id);
    }

    #[test]
    fn setext_headings_and_example_sections_keep_their_classification() {
        let sections = segment_markdown(
            "AGENTS.md",
            "Rules\n=====\nRun tests.\n\nExamples\n--------\nUse a fake command.",
        )
        .unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading, "Rules");
        assert_eq!(
            sections[0].content_class,
            InstructionContentClass::RequirementCandidate
        );
        assert_eq!(
            sections[1].content_class,
            InstructionContentClass::BackgroundOrExample
        );
    }

    #[test]
    fn binding_requirements_under_background_headings_remain_candidates() {
        let sections = segment_markdown(
            "AGENTS.md",
            "# Examples\nUse this command as a reference only.\n\n## Required in examples\nYou must preserve the API boundary.",
        )
        .unwrap();
        assert_eq!(
            sections[0].content_class,
            InstructionContentClass::BackgroundOrExample
        );
        assert_eq!(
            sections[1].content_class,
            InstructionContentClass::RequirementCandidate
        );
    }

    #[test]
    fn chunks_are_bounded_overlapped_structural_and_utf8_safe() {
        let text = format!("{}\n\n{}", "é界".repeat(5_000), "final paragraph");
        let chunks = chunk_text_with_ranges(&text);
        assert!(chunks.len() > 1);
        for &(start, end) in &chunks {
            assert!(text.is_char_boundary(start));
            assert!(text.is_char_boundary(end));
            assert!(end - start <= MAX_CHUNK_BYTES);
            assert!(text.get(start..end).is_some());
        }
        assert_eq!(chunks.first().map(|range| range.0), Some(0));
        assert_eq!(chunks.last().map(|range| range.1), Some(text.len()));
        assert!(chunks.windows(2).all(|pair| pair[1].0 < pair[0].1));
    }

    #[test]
    fn chunk_boundary_detection_recognizes_rule_shell_json_and_patch_records() {
        for (prefix, record) in [
            ("body\n", "## Next rule"),
            ("body\n", "- Next requirement"),
            ("body\n", "cat <<'EOF'"),
            ("body\n", "\"next\": {\"value\": true}"),
            ("body\n", "diff --git a/file b/file"),
            ("body\n", "@@ -1,2 +1,2 @@"),
        ] {
            let text = format!("{prefix}{record}\nrest");
            assert_eq!(structural_line_boundary(&text), Some(prefix.len()));
        }
    }

    #[test]
    fn chunking_keeps_short_command_path_and_query_envelopes_whole() {
        for envelope in [
            "command: cargo test",
            "path: crates/local/src/lib.rs",
            "query: symbol=ChunkInput scope=src",
        ] {
            assert_eq!(chunk_text_with_ranges(envelope), vec![(0, envelope.len())]);
        }
    }

    #[test]
    fn common_agent_instruction_layouts_keep_rules_conditions_and_examples() {
        let cases: &[(&str, &str, usize, &[&str])] = &[
            (
                "plain paragraphs under a heading",
                "# Code style\nUse four spaces for indentation.\n\nDo not add a second formatter.",
                1,
                &["Use four spaces", "Do not add a second formatter"],
            ),
            (
                "bullets with nested conditions",
                "## Testing\nRun focused tests before release.\n\n- Test changed code.\n  - Exception: docs-only changes need no code test.\n- Keep tests deterministic.",
                2,
                &[
                    "Run focused tests",
                    "Exception: docs-only",
                    "Keep tests deterministic",
                ],
            ),
            (
                "numbered requirements",
                "# Release\n1. Get approval before publishing.\n2. Publish only after tests pass.",
                2,
                &["Get approval", "Publish only after tests pass"],
            ),
            (
                "checkbox requirements",
                "# Review\n- [ ] Add a regression test.\n- [x] Update the release note.",
                2,
                &["[ ] Add a regression test", "[x] Update the release note"],
            ),
            (
                "fenced examples",
                "# Commands\nRun the project test command.\n\n```sh\n# This heading is an example\nprintf test\n```\nKeep the command result private.",
                1,
                &[
                    "Run the project test command",
                    "# This heading is an example",
                    "Keep the command result private",
                ],
            ),
            (
                "example heading",
                "# Examples\nUse this command as a reference only.",
                1,
                &["Use this command as a reference only"],
            ),
        ];

        for (name, markdown, expected_sections, fragments) in cases.iter().copied() {
            let sections = segment_markdown("AGENTS.md", markdown).unwrap();
            assert_eq!(sections.len(), expected_sections, "{name}");
            for fragment in fragments {
                assert!(
                    sections
                        .iter()
                        .any(|section| section.text.contains(fragment)),
                    "{name} lost {fragment:?}"
                );
            }
        }
    }

    #[test]
    fn yaml_frontmatter_stays_with_rule_text_and_keeps_source_line_numbers() {
        let text = "---\npaths: \"src/api/**/*.rs\"\n---\n# API rules\n\n- Validate request input.\n- Return the standard error format.";
        let snapshot = snapshot_from_text(
            "rules/api.md",
            text.to_owned(),
            InstructionProvenance::CurrentFileComparison,
            InstructionScope::Conditional,
        )
        .unwrap();

        assert_eq!(snapshot.text, text);
        assert_eq!(snapshot.sections.len(), 2);
        assert_eq!(snapshot.sections[0].start_line, 6);
        assert_eq!(snapshot.sections[1].start_line, 7);
        assert!(
            snapshot
                .sections
                .iter()
                .all(|section| { section.text.contains("paths: \"src/api/**/*.rs\"") })
        );
        assert!(snapshot.sections[0].text.contains("Validate request input"));
        assert!(
            snapshot.sections[1]
                .text
                .contains("Return the standard error format")
        );

        let changed_scope = snapshot_from_text(
            "rules/api.md",
            text.replace("src/api", "src/internal"),
            InstructionProvenance::CurrentFileComparison,
            InstructionScope::Conditional,
        )
        .unwrap();
        assert_ne!(snapshot.digest, changed_scope.digest);
    }
}
