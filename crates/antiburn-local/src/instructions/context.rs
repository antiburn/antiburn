use serde::{Deserialize, Serialize};

/// Exact current file text does not prove historical activation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentInstruction {
    pub path: String,
    pub content: String,
    pub observed_at: String,
    pub scope: CurrentInstructionScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentInstructionScope {
    pub root: Option<String>,
    pub globs: Vec<String>,
}

/// A configured definition, not proof of effective or historical availability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentSkill {
    pub name: String,
    pub description: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentContextRoot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Vec<CurrentInstruction>>,
    pub instructions_complete: bool,
    pub instructions_limits: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<CurrentSkill>>,
    pub skills_complete: bool,
    pub skills_limits: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_revision: Option<String>,
}

/// Hosts freeze this value before candidate preparation and reuse it on retry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentContextSnapshot {
    pub global: CurrentContextRoot,
    pub repo: CurrentContextRoot,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_lists_are_absent_and_known_empty_lists_remain_present() {
        let mut snapshot = CurrentContextSnapshot::default();
        snapshot.global.instructions = Some(Vec::new());
        let json = serde_json::to_value(snapshot).unwrap();
        assert_eq!(json["global"]["instructions"], serde_json::json!([]));
        assert!(json["repo"].get("instructions").is_none());
        assert!(json["global"].get("skills").is_none());
    }
}
