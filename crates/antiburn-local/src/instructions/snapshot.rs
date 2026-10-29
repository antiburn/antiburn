use crate::digest::sha256_hex;
use serde::{Deserialize, Serialize};

pub const MAX_INSTRUCTION_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionScope {
    Global,
    Project,
    Nested,
    Conditional,
    Unknown,
}

/// A current file observation. It does not prove historical activation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentInstructionSnapshot {
    pub id: String,
    pub digest: String,
    pub source: String,
    pub scope: InstructionScope,
    pub text: String,
    pub imports: Vec<String>,
    pub limitations: Vec<String>,
}

pub(super) fn snapshot_from_text(
    source: String,
    text: String,
    scope: InstructionScope,
) -> CurrentInstructionSnapshot {
    let digest = sha256_hex(text.as_bytes());
    let id = sha256_hex(format!("{source}\0{digest}").as_bytes());
    CurrentInstructionSnapshot {
        id,
        digest,
        source,
        scope,
        text,
        imports: Vec::new(),
        limitations: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_identity_binds_source_and_exact_text() {
        let text = "# Rules\nUse bounded reads.\n";
        let snapshot = snapshot_from_text(
            "project:AGENTS.md".into(),
            text.into(),
            InstructionScope::Project,
        );
        assert_eq!(snapshot.text, text);
        assert_eq!(snapshot.digest, sha256_hex(text.as_bytes()));
        assert_eq!(
            snapshot.id,
            sha256_hex(format!("project:AGENTS.md\0{}", snapshot.digest).as_bytes())
        );
        let other_source = snapshot_from_text(
            "home:AGENTS.md".into(),
            text.into(),
            InstructionScope::Global,
        );
        assert_eq!(snapshot.digest, other_source.digest);
        assert_ne!(snapshot.id, other_source.id);
        let changed = snapshot_from_text(
            snapshot.source.clone(),
            text.trim_end().into(),
            snapshot.scope,
        );
        assert_ne!(snapshot.digest, changed.digest);
        assert_ne!(snapshot.id, changed.id);
    }
}
