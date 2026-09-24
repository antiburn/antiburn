//! Instruction snapshots and structure-aware Markdown segmentation.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_INSTRUCTION_BYTES: usize = 128 * 1024;
pub const MAX_RULE_SECTION_BYTES: usize = 16 * 1024;
pub const MAX_INSTRUCTION_SECTIONS: usize = 256;

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
    TooManySections,
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

/// Split Markdown at heading boundaries while retaining nested list and
/// exception text with its parent section. Oversized sections stay explicit.
pub fn segment_markdown(
    source: &str,
    markdown: &str,
) -> Result<Vec<InstructionRuleSection>, MarkdownLimit> {
    if markdown.len() > MAX_INSTRUCTION_BYTES {
        return Err(MarkdownLimit::FileTooLarge);
    }

    let mut sections = Vec::new();
    let mut heading_stack: Vec<(usize, String)> = Vec::new();
    let mut body = String::new();
    let mut start_line = 1u32;
    let mut in_fence = false;

    let emit = |sections: &mut Vec<InstructionRuleSection>,
                heading_stack: &[(usize, String)],
                body: &str,
                start_line: u32,
                end_line: u32|
     -> Result<(), MarkdownLimit> {
        if body.trim().is_empty() {
            return Ok(());
        }
        if sections.len() >= MAX_INSTRUCTION_SECTIONS {
            return Err(MarkdownLimit::TooManySections);
        }
        let heading = heading_stack
            .iter()
            .map(|(_, title)| title.as_str())
            .collect::<Vec<_>>()
            .join(" / ");
        let heading_lower = heading.to_ascii_lowercase();
        let content_class = if ["example", "background", "context", "reference"]
            .iter()
            .any(|marker| heading_lower.contains(marker))
        {
            InstructionContentClass::BackgroundOrExample
        } else {
            InstructionContentClass::RequirementCandidate
        };
        let digest = sha256_hex(body.as_bytes());
        let identity = format!("{source}\0{digest}\0{start_line}\0{end_line}\0{heading}");
        sections.push(InstructionRuleSection {
            id: sha256_hex(identity.as_bytes()),
            heading,
            content_class,
            text: body.to_owned(),
            start_line,
            end_line,
            evaluable: body.len() <= MAX_RULE_SECTION_BYTES,
        });
        Ok(())
    };

    let lines: Vec<&str> = markdown.lines().collect();
    let mut skip_setext_underline = false;
    for (index, line) in lines.iter().enumerate() {
        if skip_setext_underline {
            skip_setext_underline = false;
            continue;
        }
        let line_number = index as u32 + 1;
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
            emit(
                &mut sections,
                &heading_stack,
                &body,
                start_line,
                line_number.saturating_sub(1),
            )?;
            body.clear();
            while heading_stack
                .last()
                .is_some_and(|(parent_level, _)| *parent_level >= level)
            {
                heading_stack.pop();
            }
            heading_stack.push((level, title.to_owned()));
            start_line = line_number;
            if setext.is_some() {
                skip_setext_underline = true;
            }
        } else {
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
        start_line,
        lines.len() as u32,
    )?;
    Ok(sections)
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
}
