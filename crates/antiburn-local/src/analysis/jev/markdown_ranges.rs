use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkdownBlockKind {
    Heading,
    Paragraph,
    List,
    Item,
    BlockQuote,
    CodeBlock,
    HtmlBlock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MarkdownBlock {
    pub(crate) range: Range<usize>,
    pub(crate) kind: MarkdownBlockKind,
    pub(crate) parent: Option<usize>,
    pub(crate) section: Option<usize>,
    pub(crate) quoted: bool,
    pub(crate) list_depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MarkdownHeading {
    pub(crate) range: Range<usize>,
    pub(crate) level: usize,
    pub(crate) title: String,
    pub(crate) section: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MarkdownSection {
    pub(crate) range: Range<usize>,
    pub(crate) body_start: usize,
    pub(crate) level: usize,
    pub(crate) parent: Option<usize>,
}

/// Parse structure once. Evidence uses slices of `source`, not rendered events.
pub(crate) struct MarkdownRanges<'a> {
    pub(crate) source: &'a str,
    pub(crate) blocks: Vec<MarkdownBlock>,
    pub(crate) headings: Vec<MarkdownHeading>,
    pub(crate) sections: Vec<MarkdownSection>,
    pub(crate) prose_ranges: Vec<Range<usize>>,
}

impl<'a> MarkdownRanges<'a> {
    pub(crate) fn parse(source: &'a str) -> Self {
        let mut parsed = Self {
            source,
            blocks: Vec::new(),
            headings: Vec::new(),
            sections: Vec::new(),
            prose_ranges: Vec::new(),
        };
        let mut blocks = Vec::new();
        let mut sections: Vec<usize> = Vec::new();
        let mut quote_depth = 0usize;
        let mut list_depth = 0usize;
        let mut code_depth = 0usize;
        let mut heading: Option<MarkdownHeading> = None;
        for (event, range) in Parser::new_ext(source, Options::empty()).into_offset_iter() {
            match event {
                Event::Start(tag) => {
                    if let Tag::Heading { level, .. } = tag
                        && quote_depth == 0
                        && list_depth == 0
                    {
                        let level = heading_level(level);
                        while sections
                            .last()
                            .is_some_and(|&parent| parsed.sections[parent].level >= level)
                        {
                            let index = sections.pop().expect("open section");
                            parsed.sections[index].range.end = range.start;
                        }
                        let index = parsed.sections.len();
                        parsed.sections.push(MarkdownSection {
                            range: range.start..source.len(),
                            body_start: range.end,
                            level,
                            parent: sections.last().copied(),
                        });
                        sections.push(index);
                        heading = Some(MarkdownHeading {
                            range: range.clone(),
                            level,
                            title: String::new(),
                            section: index,
                        });
                    }
                    if matches!(tag, Tag::BlockQuote(_)) {
                        quote_depth += 1;
                    }
                    if matches!(tag, Tag::List(_)) {
                        list_depth += 1;
                    }
                    if matches!(tag, Tag::CodeBlock(_)) {
                        code_depth += 1;
                    }
                    if let Some(kind) = block_kind(&tag) {
                        let index = parsed.blocks.len();
                        parsed.blocks.push(MarkdownBlock {
                            range,
                            kind,
                            parent: blocks.last().copied(),
                            section: sections.last().copied(),
                            quoted: quote_depth > 0,
                            list_depth,
                        });
                        blocks.push(index);
                    }
                }
                Event::End(tag) => {
                    if ends_block(tag) {
                        let index = blocks.pop().expect("parser closes an open block");
                        parsed.blocks[index].range.end = range.end;
                    }
                    if matches!(tag, TagEnd::Heading(_))
                        && let Some(value) = heading.take()
                    {
                        parsed.headings.push(value);
                    }
                    if matches!(tag, TagEnd::BlockQuote(_)) {
                        quote_depth -= 1;
                    }
                    if matches!(tag, TagEnd::List(_)) {
                        list_depth -= 1;
                    }
                    if matches!(tag, TagEnd::CodeBlock) {
                        code_depth -= 1;
                    }
                }
                Event::Text(text) => {
                    if let Some(value) = heading.as_mut() {
                        value.title.push_str(&text);
                    }
                    if quote_depth == 0 && code_depth == 0 {
                        parsed.prose_ranges.push(range);
                    }
                }
                Event::Code(text) => {
                    if let Some(value) = heading.as_mut() {
                        value.title.push_str(&text);
                    }
                }
                _ => {}
            }
        }
        parsed
    }
}

fn block_kind(tag: &Tag<'_>) -> Option<MarkdownBlockKind> {
    match tag {
        Tag::Heading { .. } => Some(MarkdownBlockKind::Heading),
        Tag::Paragraph => Some(MarkdownBlockKind::Paragraph),
        Tag::List(_) => Some(MarkdownBlockKind::List),
        Tag::Item => Some(MarkdownBlockKind::Item),
        Tag::BlockQuote(_) => Some(MarkdownBlockKind::BlockQuote),
        Tag::CodeBlock(_) => Some(MarkdownBlockKind::CodeBlock),
        Tag::HtmlBlock => Some(MarkdownBlockKind::HtmlBlock),
        _ => None,
    }
}

fn ends_block(tag: TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Heading(_)
            | TagEnd::Paragraph
            | TagEnd::List(_)
            | TagEnd::Item
            | TagEnd::BlockQuote(_)
            | TagEnd::CodeBlock
            | TagEnd::HtmlBlock
    )
}

fn heading_level(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_keep_source_offsets_and_ignore_fenced_quoted_and_list_examples() {
        let source =
            "# Rules\n\n```md\n# Example\n```\n\n> # Quoted\n\n- ## List heading\n\n## Scope\n";
        let parsed = MarkdownRanges::parse(source);
        assert_eq!(parsed.headings.len(), 2);
        assert_eq!(parsed.headings[0].title, "Rules");
        assert_eq!(
            source[parsed.headings[1].range.clone()].trim_end(),
            "## Scope"
        );
        assert_eq!(parsed.sections[1].parent, Some(0));
        assert_eq!(parsed.sections[0].range, 0..source.len());
        assert!(
            parsed
                .blocks
                .iter()
                .any(|block| block.kind == MarkdownBlockKind::Heading && block.quoted)
        );
    }

    #[test]
    fn setext_crlf_headings_and_nested_lists_keep_exact_source_ranges_and_parents() {
        let source = "Release checks\r\n---------------\r\n- Run `cargo test`.\r\n  - Unless the user declines.\r\n- Keep results private.\r\n\r\nNext\r\n----\r\nDone.\r\n";
        let parsed = MarkdownRanges::parse(source);
        assert_eq!(parsed.headings[0].title, "Release checks");
        assert_eq!(
            &source[parsed.headings[0].range.clone()],
            "Release checks\r\n---------------\r\n"
        );
        assert_eq!(parsed.sections[0].range.end, parsed.headings[1].range.start);
        assert_eq!(parsed.sections[1].parent, None);
        let nested = parsed
            .blocks
            .iter()
            .find(|block| block.kind == MarkdownBlockKind::Item && block.list_depth == 2)
            .unwrap();
        assert!(source[nested.range.clone()].contains("Unless the user declines."));
        let parent = &parsed.blocks[nested.parent.unwrap()];
        assert_eq!(parent.kind, MarkdownBlockKind::List);
        assert!(parent.range.start <= nested.range.start && parent.range.end >= nested.range.end);
        assert!(
            parsed
                .blocks
                .iter()
                .all(|block| source.get(block.range.clone()).is_some())
        );
    }

    #[test]
    fn mismatched_fences_do_not_create_prose_or_new_sections() {
        let source = "# Rules\n```md\nDo not push.\n~~~\n# Example\n";
        let parsed = MarkdownRanges::parse(source);
        assert_eq!(parsed.headings.len(), 1);
        assert!(
            !parsed
                .prose_ranges
                .iter()
                .any(|range| source[range.clone()].contains("Do not push"))
        );
        assert_eq!(
            parsed
                .blocks
                .iter()
                .find(|block| block.kind == MarkdownBlockKind::CodeBlock)
                .unwrap()
                .range
                .end,
            source.len()
        );
    }
}
