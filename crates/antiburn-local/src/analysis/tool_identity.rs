use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::interface::{ContentKind, ContentPart};

const MAX_CALLS: usize = 4096;
const MAX_IDENTITY_BYTES: usize = 512;

/// Join recorded tool identities within one source and branch.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ToolIdentityMap {
    calls: BTreeMap<String, Option<String>>,
}

impl ToolIdentityMap {
    pub(crate) fn bind_parts(&mut self, branch: Option<&str>, parts: &mut [ContentPart]) {
        for part in parts {
            let Some(id) = part.tool_call_id.as_deref() else {
                continue;
            };
            if id.is_empty()
                || id.len() > MAX_IDENTITY_BYTES
                || branch.is_some_and(|branch| branch.len() > MAX_IDENTITY_BYTES)
            {
                continue;
            }
            let key = match branch {
                Some(branch) => format!("thread:{}:{branch}{id}", branch.len()),
                None => format!("main:{id}"),
            };
            match part.kind {
                ContentKind::ToolInput => {
                    let Some(name) = part
                        .tool_name
                        .as_deref()
                        .filter(|name| !name.is_empty() && name.len() <= MAX_IDENTITY_BYTES)
                    else {
                        continue;
                    };
                    if let Some(recorded) = self.calls.get_mut(&key) {
                        if recorded.as_deref() != Some(name) {
                            *recorded = None;
                        }
                    } else if self.calls.len() < MAX_CALLS {
                        self.calls.insert(key, Some(name.to_owned()));
                    }
                }
                ContentKind::ToolResult => {
                    if let Some(recorded) = self.calls.get(&key) {
                        part.tool_name = match (&part.tool_name, recorded) {
                            (None, Some(name)) => Some(name.clone()),
                            (Some(name), Some(recorded)) if name == recorded => Some(name.clone()),
                            _ => None,
                        };
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(kind: ContentKind, name: Option<&str>, id: Option<&str>) -> ContentPart {
        ContentPart::new(kind, "synthetic")
            .with_tool_identity(name.map(str::to_owned), id.map(str::to_owned))
    }

    #[test]
    fn exact_joins_reject_conflicts_missing_ids_and_other_branches() {
        let mut identities = ToolIdentityMap::default();
        identities.bind_parts(
            None,
            &mut [part(ContentKind::ToolInput, Some("Bash"), Some("call"))],
        );
        let mut results = [
            part(ContentKind::ToolResult, None, Some("call")),
            part(ContentKind::ToolResult, None, Some("missing")),
            part(ContentKind::ToolResult, None, None),
            part(ContentKind::ToolResult, Some("Read"), Some("call")),
        ];
        identities.bind_parts(None, &mut results);
        assert_eq!(results[0].tool_name.as_deref(), Some("Bash"));
        assert!(results[1..].iter().all(|part| part.tool_name.is_none()));
        let mut child = [part(ContentKind::ToolResult, None, Some("call"))];
        identities.bind_parts(Some("child"), &mut child);
        assert!(child[0].tool_name.is_none());
        identities.bind_parts(
            None,
            &mut [part(ContentKind::ToolInput, Some("Read"), Some("call"))],
        );
        identities.bind_parts(
            None,
            &mut [part(ContentKind::ToolInput, Some("Bash"), Some("call"))],
        );
        let mut conflict = [part(ContentKind::ToolResult, None, Some("call"))];
        identities.bind_parts(None, &mut conflict);
        assert!(conflict[0].tool_name.is_none());
        let restored: ToolIdentityMap =
            serde_json::from_str(&serde_json::to_string(&identities).unwrap()).unwrap();
        assert_eq!(restored.calls, identities.calls);
    }

    #[test]
    fn capacity_and_identity_limits_do_not_evict_or_guess_joins() {
        let mut identities = ToolIdentityMap::default();
        for index in 0..MAX_CALLS + 1 {
            identities.bind_parts(
                None,
                &mut [part(
                    ContentKind::ToolInput,
                    Some("Bash"),
                    Some(&index.to_string()),
                )],
            );
        }
        assert_eq!(identities.calls.len(), MAX_CALLS);
        let mut results = [
            part(ContentKind::ToolResult, None, Some("0")),
            part(ContentKind::ToolResult, None, Some(&MAX_CALLS.to_string())),
        ];
        identities.bind_parts(None, &mut results);
        assert_eq!(results[0].tool_name.as_deref(), Some("Bash"));
        assert!(results[1].tool_name.is_none());
        identities.bind_parts(
            None,
            &mut [part(
                ContentKind::ToolInput,
                Some("Read"),
                Some(&"x".repeat(MAX_IDENTITY_BYTES + 1)),
            )],
        );
        assert_eq!(identities.calls.len(), MAX_CALLS);
    }
}
