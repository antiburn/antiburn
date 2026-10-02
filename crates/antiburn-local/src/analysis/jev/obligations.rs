//! Bounded obligation state at check-owned recorded boundaries.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationKind {
    Action,
    Prerequisite,
    Completion,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConditionEvidence {
    Selected,
    Result,
    Undefined,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObligationState {
    Pending,
    Satisfied,
    Violated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRequirement {
    Independent,
    AuthoritativeApproval,
    ApprovalClaim,
    #[default]
    Unknown,
}

impl PermissionRequirement {
    pub fn observable_without_authority(self) -> bool {
        matches!(self, Self::Independent | Self::ApprovalClaim)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadRequestOrder {
    pub required_path: String,
    pub earlier_request_id: Option<String>,
    pub later_request_id: Option<String>,
    pub history_complete: bool,
    pub paths_known: bool,
}

impl ReadRequestOrder {
    pub fn state(&self) -> ObligationState {
        if self.earlier_request_id.is_some() {
            ObligationState::Satisfied
        } else if self.history_complete && self.paths_known {
            ObligationState::Violated
        } else {
            ObligationState::Pending
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestPathScope {
    Directory(String),
    File(String),
}

impl RequestPathScope {
    pub fn matches(&self, path: &str) -> Option<bool> {
        let scope = match self {
            Self::Directory(scope) | Self::File(scope) => scope,
        };
        if [path, scope.as_str()].into_iter().any(|value| {
            value.starts_with('/')
                || value.contains(['*', '?', '$', '~', '\\', ':', '{', '}', '[', ']'])
                || value.split('/').any(|part| matches!(part, "." | ".."))
        }) {
            return None;
        }
        match self {
            Self::Directory(directory) => {
                Some(path.starts_with(&format!("{}/", directory.trim_end_matches('/'))))
            }
            Self::File(file) => Some(path == file),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_request_scopes_do_not_use_substrings_or_resolve_aliases() {
        let scope = RequestPathScope::Directory("src/ui/".to_owned());
        assert_eq!(scope.matches("src/ui/panel.rs"), Some(true));
        assert_eq!(scope.matches("src/uikit/panel.rs"), Some(false));
        assert_eq!(scope.matches("./src/ui/panel.rs"), None);
        assert_eq!(scope.matches("src/ui/../storage.rs"), None);
        let scope = RequestPathScope::File("src/ui/panel.rs".to_owned());
        assert_eq!(scope.matches("src/ui/panel.rs"), Some(true));
        assert_eq!(scope.matches("src/ui/Panel.rs"), Some(false));
        for value in ["./src/ui", "src/*", "../src", "$ROOT/src"] {
            assert_eq!(
                RequestPathScope::Directory(value.to_owned()).matches("src/ui/panel.rs"),
                None
            );
        }
    }
}
