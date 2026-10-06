//! Lossless file and hunk ranges for selected unified and apply-patch text.

use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum EditPathOperation {
    Add { path: String },
    Update { path: String },
    Delete { path: String },
    Move { from: String, to: String },
}

pub fn apply_patch_path_operations(text: &str) -> Vec<EditPathOperation> {
    let mut operations = Vec::new();
    for line in text.lines() {
        if operations.len() == 256 {
            break;
        }
        let path = |prefix| {
            line.strip_prefix(prefix)
                .filter(|value| !value.is_empty() && value.len() <= 4096)
                .map(str::to_owned)
        };
        if let Some(path) = path("*** Add File: ") {
            operations.push(EditPathOperation::Add { path });
        } else if let Some(path) = path("*** Update File: ") {
            operations.push(EditPathOperation::Update { path });
        } else if let Some(path) = path("*** Delete File: ") {
            operations.push(EditPathOperation::Delete { path });
        } else if let Some(to) = path("*** Move to: ")
            && let Some(EditPathOperation::Update { path }) = operations.last()
        {
            let from = path.clone();
            *operations.last_mut().expect("an update precedes the move") =
                EditPathOperation::Move { from, to };
        }
    }
    operations
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditLineKind {
    Added,
    Removed,
    Unchanged,
    Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditLine {
    pub range: Range<usize>,
    pub kind: EditLineKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditHunk {
    pub range: Range<usize>,
    pub file_header: Option<Range<usize>>,
    pub lines: Vec<EditLine>,
}

pub fn edit_hunks(text: &str) -> Vec<EditHunk> {
    let mut hunks: Vec<EditHunk> = Vec::new();
    let mut file_header = None;
    let mut offset = 0;
    let mut in_hunk = false;
    let mut remaining = None;
    for line in text.split_inclusive('\n') {
        let range = offset..offset + line.len();
        let file = line.starts_with("diff --git ")
            || line.starts_with("*** Update File: ")
            || line.starts_with("*** Add File: ")
            || line.starts_with("*** Delete File: ")
            || (!in_hunk && line.starts_with("--- "));
        if file {
            file_header = Some(range.clone());
            in_hunk = false;
            remaining = None;
        }
        if hunks.is_empty() || file || line.starts_with("@@") {
            hunks.push(EditHunk {
                range: range.clone(),
                file_header: file_header.clone(),
                lines: Vec::new(),
            });
        }
        let kind = if line.starts_with('+') && (in_hunk || !line.starts_with("+++ ")) {
            EditLineKind::Added
        } else if line.starts_with('-') && (in_hunk || !line.starts_with("--- ")) {
            EditLineKind::Removed
        } else if line.starts_with(' ') {
            EditLineKind::Unchanged
        } else {
            EditLineKind::Metadata
        };
        let hunk = hunks.last_mut().expect("a line starts a hunk");
        hunk.range.end = range.end;
        hunk.lines.push(EditLine { range, kind });
        if line.starts_with("@@") {
            in_hunk = true;
            remaining = unified_line_counts(line);
        } else if let Some((old, new)) = &mut remaining {
            match kind {
                EditLineKind::Added => *new = new.saturating_sub(1),
                EditLineKind::Removed => *old = old.saturating_sub(1),
                EditLineKind::Unchanged => {
                    *old = old.saturating_sub(1);
                    *new = new.saturating_sub(1);
                }
                EditLineKind::Metadata => {}
            }
            if *old == 0 && *new == 0 {
                in_hunk = false;
            }
        }
        offset += line.len();
    }
    hunks
}

fn unified_line_counts(header: &str) -> Option<(usize, usize)> {
    let mut parts = header.strip_prefix("@@ ")?.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    if parts.next()? != "@@" {
        return None;
    }
    let count = |range: &str| -> Option<usize> {
        let (start, count) = range.split_once(',').unwrap_or((range, "1"));
        start.parse::<usize>().ok()?;
        count.parse().ok()
    };
    Some((count(old)?, count(new)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn patch_path_operations_preserve_direction_without_body_text() {
        let text = "*** Begin Patch\n*** Update File: old.rs\n*** Move to: new.rs\n@@\n+// *** Delete File: secret.rs\n*** Delete File: removed.rs\n*** Add File: added.rs\n+PRIVATE_BODY\n*** End Patch";
        assert_eq!(
            apply_patch_path_operations(text),
            vec![
                EditPathOperation::Move {
                    from: "old.rs".to_owned(),
                    to: "new.rs".to_owned()
                },
                EditPathOperation::Delete {
                    path: "removed.rs".to_owned()
                },
                EditPathOperation::Add {
                    path: "added.rs".to_owned()
                },
            ]
        );
        assert!(
            !serde_json::to_string(&apply_patch_path_operations(text))
                .unwrap()
                .contains("PRIVATE_BODY")
        );
    }
    #[test]
    fn header_like_code_is_not_a_new_file_or_metadata() {
        let text = "--- a/first\n+++ b/first\n@@ -1 +1 @@\n--- old_code\n+++ new_code\n--- a/second\n+++ b/second\n@@ -1 +1 @@\n-x\n+y\n";
        let hunks = edit_hunks(text);
        assert_eq!(hunks[1].lines[1].kind, EditLineKind::Removed);
        assert_eq!(hunks[1].lines[2].kind, EditLineKind::Added);
        assert_eq!(
            &text[hunks[2].file_header.clone().unwrap()],
            "--- a/second\n"
        );
        assert_eq!(
            hunks
                .iter()
                .map(|hunk| &text[hunk.range.clone()])
                .collect::<String>(),
            text
        );
    }
    #[test]
    fn preserves_multifile_hunks_and_all_utf8_lines() {
        let text = "*** Begin Patch\n*** Update File: a.rs\n@@ first\n-old\n+新\n same\n@@ second\n+next\n*** Update File: b.rs\n@@\n-last\n+new\n*** End Patch\n";
        let hunks = edit_hunks(text);
        let rebuilt: String = hunks.iter().map(|hunk| &text[hunk.range.clone()]).collect();
        assert_eq!(rebuilt, text);
        let added: Vec<_> = hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| line.kind == EditLineKind::Added)
            .map(|line| &text[line.range.clone()])
            .collect();
        assert_eq!(added, ["+新\n", "+next\n", "+new\n"]);
        assert!(hunks.iter().any(|hunk| {
            hunk.file_header
                .as_ref()
                .is_some_and(|range| text[range.clone()].contains("b.rs"))
        }));
    }
    #[test]
    fn unified_headers_are_metadata_and_empty_input_has_no_hunks() {
        assert!(edit_hunks("").is_empty());
        let text = "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-x\n+y";
        let hunks = edit_hunks(text);
        assert_eq!(hunks[0].lines[0].kind, EditLineKind::Metadata);
        assert_eq!(hunks[0].lines[1].kind, EditLineKind::Metadata);
        assert_eq!(hunks[1].lines[0].kind, EditLineKind::Metadata);
        assert_eq!(hunks[1].lines[2].kind, EditLineKind::Added);
    }
}
