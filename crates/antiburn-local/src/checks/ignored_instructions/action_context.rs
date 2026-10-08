//! Exact request context for file operations and a bounded here-document shape.

use super::CandidateComparison;
use crate::analysis::jev::edit_hunks::EditPathOperation;
use serde::{Deserialize, Serialize};

pub(super) fn is_assistant_text(kind: &str) -> bool {
    matches!(kind, "assistant" | "assistant_text")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathChangePolicy {
    AllChanges,
    Delete,
    MoveOut,
    MoveIn,
    #[default]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RequestedPathChange {
    Create,
    Modify,
    Delete,
    MoveFrom,
    MoveTo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PathChange<'a> {
    pub path: &'a str,
    pub change: RequestedPathChange,
}

pub(super) fn path_changes(operations: &[EditPathOperation]) -> Vec<PathChange<'_>> {
    operations
        .iter()
        .flat_map(|operation| match operation {
            EditPathOperation::Add { path } => vec![PathChange {
                path,
                change: RequestedPathChange::Create,
            }],
            EditPathOperation::Update { path } => vec![PathChange {
                path,
                change: RequestedPathChange::Modify,
            }],
            EditPathOperation::Delete { path } => vec![PathChange {
                path,
                change: RequestedPathChange::Delete,
            }],
            EditPathOperation::Move { from, to } => vec![
                PathChange {
                    path: from,
                    change: RequestedPathChange::MoveFrom,
                },
                PathChange {
                    path: to,
                    change: RequestedPathChange::MoveTo,
                },
            ],
        })
        .collect()
}

pub(super) fn rule_path_candidates(text: &str) -> Vec<String> {
    let mut paths = crate::analysis::jev::exact_facts::reference_path_candidates(text);
    paths.extend(
        text.split(|character: char| {
            character.is_whitespace()
                || matches!(character, '`' | '\'' | '"' | '(' | ')' | ',' | ';')
        })
        .filter(|token| token.contains('\\'))
        .map(|token| token.trim_end_matches(['.', ':']).to_owned()),
    );
    paths.sort();
    paths.dedup();
    paths
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct HereDocumentContext {
    pub header: String,
    pub delimiter: String,
    pub body_expands: bool,
    pub selected_range_intersects_input: bool,
}

pub(super) fn here_document_context(
    command: &str,
    source_text: &str,
    comparison: &CandidateComparison,
) -> Option<HereDocumentContext> {
    if let Ok(serde_json::Value::Object(envelope)) = serde_json::from_str(source_text)
        && let Some(shell) = envelope.get("shell")
    {
        let name = shell.as_str()?.rsplit('/').next()?;
        if !matches!(name, "sh" | "bash" | "dash" | "zsh" | "ksh") {
            return None;
        }
    }
    let mut header_start = 0;
    for line in command.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            if trimmed.ends_with('\\') {
                return None;
            }
            header_start += line.len();
        } else {
            break;
        }
    }
    let (header, rest) = command.get(header_start..)?.split_once('\n')?;
    if header.len() > 1024 || header.matches("<<").count() != 1 {
        return None;
    }
    let (invocation, marker) = header.split_once("<<")?;
    let invocation = invocation.trim();
    if invocation.is_empty()
        || !invocation.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '-' | '.' | '/' | ' ' | '\t')
        })
    {
        return None;
    }
    let marker = marker.trim();
    let (strip_tabs, marker) = marker
        .strip_prefix('-')
        .map(|marker| (true, marker))
        .unwrap_or((false, marker));
    let quoted = marker.starts_with(['\'', '"']);
    let delimiter = if quoted {
        let quote = marker.chars().next()?;
        marker.strip_prefix(quote)?.strip_suffix(quote)?
    } else {
        marker
    };
    if delimiter.is_empty()
        || delimiter.len() > 64
        || !delimiter
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return None;
    }
    let body_start = header_start + header.len() + 1;
    let mut body_end = body_start;
    let mut closing = None;
    for line in rest.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = if strip_tabs {
            content.trim_start_matches('\t')
        } else {
            content
        };
        if content == delimiter {
            closing = Some(body_end + line.len());
            break;
        }
        body_end += line.len();
    }
    if !command.get(closing?..)?.trim().is_empty() {
        return None;
    }
    let (encoded_start, encoded_end) = if source_text == command {
        (body_start, body_end)
    } else {
        let encoded = serde_json::to_string(command).ok()?;
        let mut occurrences = source_text.match_indices(&encoded);
        let (command_start, _) = occurrences.next()?;
        if occurrences.next().is_some() {
            return None;
        }
        (
            command_start + 1 + serde_json::to_string(&command[..body_start]).ok()?.len() - 2,
            command_start + 1 + serde_json::to_string(&command[..body_end]).ok()?.len() - 2,
        )
    };
    Some(HereDocumentContext {
        header: header.to_owned(),
        delimiter: delimiter.to_owned(),
        body_expands: !quoted,
        selected_range_intersects_input: comparison.action_text_start < encoded_end
            && comparison.action_text_end > encoded_start,
    })
}

#[cfg(test)]
mod tests;
