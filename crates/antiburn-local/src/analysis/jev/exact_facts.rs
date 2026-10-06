//! Exact facts from selected evidence. These facts do not prove execution.

use super::{JevInputField, JevNormalizedFields};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExactActionFacts {
    pub tool_name: Option<String>,
    pub paths: Vec<String>,
    pub command: Option<String>,
    pub search_query: Option<String>,
    pub edit_operations: Vec<super::edit_hunks::EditPathOperation>,
    pub malformed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExactIdentifierFact {
    pub identifier: String,
    pub matches_recorded_path: bool,
    pub matches_recorded_command: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiteralPolicy {
    ConstructBan,
    CommandBan,
    ToolBan,
    ResponseLiteral,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiteralPolicyBinding {
    pub identifier: String,
    pub policy: LiteralPolicy,
    pub exact_match: Option<bool>,
}

pub fn reference_path_candidates(reference: &str) -> Vec<String> {
    let mut paths = reference
        .split(|character: char| {
            character.is_whitespace()
                || matches!(character, '`' | '\'' | '"' | '(' | ')' | ',' | ';')
        })
        .map(|token| token.trim_end_matches(['.', ':']))
        .filter(|token| !token.is_empty() && (token.contains('/') || token.contains('.')))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

pub fn reference_identifier_facts(
    reference: &str,
    action: &ExactActionFacts,
) -> Vec<ExactIdentifierFact> {
    let mut facts = Vec::new();
    let mut offset = 0;
    let mut fenced = false;
    while let Some(relative) = reference[offset..].find('`') {
        let start = offset + relative;
        let width = reference[start..]
            .bytes()
            .take_while(|byte| *byte == b'`')
            .count();
        offset = start + width;
        if width >= 3 {
            fenced = !fenced;
            continue;
        }
        if fenced || width != 1 {
            continue;
        }
        let Some(end) = reference[offset..]
            .find('`')
            .map(|relative| offset + relative)
        else {
            break;
        };
        let identifier = &reference[offset..end];
        if !identifier.is_empty() && !identifier.contains('\n') {
            facts.push(ExactIdentifierFact {
                identifier: identifier.to_owned(),
                matches_recorded_path: action.paths.iter().any(|path| path == identifier),
                matches_recorded_command: action.command.as_deref() == Some(identifier),
            });
        }
        offset = end + 1;
    }
    facts
}

impl ExactActionFacts {
    pub fn from_store(
        store: &super::JevEvidenceStore,
        source_id: &str,
        tool_name: Option<&str>,
    ) -> Self {
        let fields = JevNormalizedFields {
            category: None,
            values: [
                JevInputField::BashCommandInput,
                JevInputField::FileEditPath,
                JevInputField::ReadFilePath,
                JevInputField::SearchFilesQuery,
            ]
            .into_iter()
            .filter_map(|field| {
                store
                    .get(source_id, field)
                    .map(|text| (field, text.to_owned()))
            })
            .collect(),
            malformed: false,
        };
        Self::from_selected(tool_name, Some(&fields))
    }
    pub fn from_selected(tool_name: Option<&str>, fields: Option<&JevNormalizedFields>) -> Self {
        let mut facts = Self {
            tool_name: tool_name.map(str::to_owned),
            ..Self::default()
        };
        let Some(fields) = fields else { return facts };
        facts.malformed = fields.malformed;
        facts.command = fields
            .values
            .get(&JevInputField::BashCommandInput)
            .and_then(
                |text| match serde_json::from_str::<serde_json::Value>(text) {
                    Ok(serde_json::Value::Object(value)) => value
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    _ => Some(text.clone()),
                },
            );
        facts.search_query = fields.values.get(&JevInputField::SearchFilesQuery).cloned();
        for field in [JevInputField::FileEditPath, JevInputField::ReadFilePath] {
            if let Some(value) = fields.values.get(&field)
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(value)
            {
                if let Some(path) = value.get("path").and_then(serde_json::Value::as_str) {
                    facts.paths.push(path.to_owned());
                }
                if let Some(paths) = value.get("paths").and_then(serde_json::Value::as_array) {
                    facts.paths.extend(
                        paths
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_owned),
                    );
                }
                if field == JevInputField::FileEditPath
                    && let Some(operations) = value.get("operations")
                    && let Ok(operations) = serde_json::from_value::<
                        Vec<super::edit_hunks::EditPathOperation>,
                    >(operations.clone())
                {
                    facts.edit_operations = operations;
                }
            }
        }
        facts.paths.sort();
        facts.paths.dedup();
        facts
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordedOrder {
    Before,
    Same,
    After,
    Unknown,
}

pub fn recorded_order(
    left_branch: &str,
    left_scope: &str,
    left: u64,
    right_branch: &str,
    right_scope: &str,
    right: u64,
) -> RecordedOrder {
    if left_branch != right_branch || left_scope != right_scope {
        return RecordedOrder::Unknown;
    }
    match left.cmp(&right) {
        std::cmp::Ordering::Less => RecordedOrder::Before,
        std::cmp::Ordering::Equal => RecordedOrder::Same,
        std::cmp::Ordering::Greater => RecordedOrder::After,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_identifiers_do_not_match_substrings_or_fenced_examples() {
        let facts = ExactActionFacts {
            paths: vec!["src/a.rs".to_owned()],
            command: Some("git commit -s".to_owned()),
            ..Default::default()
        };
        let matches = reference_identifier_facts(
            "Use `a.rs`, `src/a.rs`, and `git commit -s`.\n```\n`example`\n```",
            &facts,
        );
        assert_eq!(matches.len(), 3);
        assert!(!matches[0].matches_recorded_path);
        assert!(matches[1].matches_recorded_path);
        assert!(matches[2].matches_recorded_command);
    }
    #[test]
    fn exact_facts_use_normalized_requests_without_outputs_or_edit_content() {
        let mut store = super::super::JevEvidenceStore::for_publication(1);
        store
            .insert(
                "edit",
                JevInputField::FileEditPath,
                r#"{"paths":["old.rs","new.rs","old.rs"]}"#.to_owned(),
            )
            .unwrap();
        store
            .insert(
                "edit",
                JevInputField::FileEditContent,
                "EXCLUDED_EDIT_BODY".to_owned(),
            )
            .unwrap();
        store
            .insert(
                "edit",
                JevInputField::BashCommandOutput,
                "EXCLUDED_RESULT".to_owned(),
            )
            .unwrap();
        let facts = ExactActionFacts::from_store(&store, "edit", Some("apply_patch"));
        assert_eq!(facts.paths, ["new.rs", "old.rs"]);
        let serialized = serde_json::to_string(&facts).unwrap();
        assert!(!serialized.contains("EXCLUDED"));
        store
            .insert(
                "raw",
                JevInputField::BashCommandInput,
                "git commit -s".to_owned(),
            )
            .unwrap();
        assert_eq!(
            ExactActionFacts::from_store(&store, "raw", Some("bash"))
                .command
                .as_deref(),
            Some("git commit -s")
        );
        store
            .insert(
                "context",
                JevInputField::BashCommandInput,
                r#"{"command":"cargo test","cwd":"/project"}"#.to_owned(),
            )
            .unwrap();
        assert_eq!(
            ExactActionFacts::from_store(&store, "context", Some("bash"))
                .command
                .as_deref(),
            Some("cargo test")
        );
    }
    #[test]
    fn order_requires_the_same_recorded_branch_and_scope() {
        assert_eq!(
            recorded_order("a", "main", 9, "a", "main", 10),
            RecordedOrder::Before
        );
        assert_eq!(
            recorded_order("a", "main", 9, "b", "main", 10),
            RecordedOrder::Unknown
        );
        assert_eq!(
            recorded_order("a", "child", 9, "a", "main", 10),
            RecordedOrder::Unknown
        );
    }
}
