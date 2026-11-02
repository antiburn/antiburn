//! Current observed inventories and recorded requester attribution.

use serde::{Deserialize, Serialize};

/// Proven generating or requesting facts for one recorded scope.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedAttribution {
    /// Exact recorded provider/model identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Recorded fast-mode evidence. Missing does not mean standard mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_mode: Option<bool>,
}

/// Derives each uniform fact independently from a complete, nonempty request set.
#[must_use]
pub fn uniform_attribution(
    requests: &[ObservedAttribution],
    complete: bool,
) -> ObservedAttribution {
    let Some(first) = requests.first().filter(|_| complete) else {
        return ObservedAttribution::default();
    };
    ObservedAttribution {
        model: first
            .model
            .as_ref()
            .filter(|model| {
                crate::filters::is_model_identifier(model)
                    && requests
                        .iter()
                        .all(|request| request.model.as_ref() == Some(model))
            })
            .cloned(),
        fast_mode: first.fast_mode.filter(|mode| {
            requests
                .iter()
                .all(|request| request.fast_mode == Some(*mode))
        }),
    }
}

/// Current instruction text and its supported scope at collection time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionRecord {
    /// Absolute instruction file path.
    pub path: String,
    /// Observed instruction text, not proof of historical activation.
    pub content: String,
    /// Supported scope of this instruction.
    pub scope: InstructionScope,
    /// RFC 3339 collection time.
    pub observed_at: String,
}

/// A supported current instruction scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionScope {
    /// Absolute scope root, or null for global instructions with no globs.
    pub root: Option<String>,
    /// Supported repository-relative scope patterns.
    pub globs: Vec<String>,
}

/// Current configured skill metadata. This record does not contain the skill body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRecord {
    /// Declared skill name. Duplicate names remain distinct records.
    pub name: String,
    /// Declared description, or empty text when absent.
    pub description: String,
    /// Absolute skill definition path.
    pub path: String,
}

/// One bounded current inventory. Missing arrays are unavailable, not empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventorySnapshot {
    /// Known instructions, including useful entries from partial scans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Vec<InstructionRecord>>,
    /// Whether the supported instruction scan completed without omissions.
    #[serde(default)]
    pub instructions_complete: bool,
    /// Explicit instruction collection and scope limits.
    #[serde(default)]
    pub instructions_limits: Vec<String>,
    /// Known configured skills, not an inferred effective agent inventory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<SkillRecord>>,
    /// Whether the supported skill scan completed without omissions.
    #[serde(default)]
    pub skills_complete: bool,
    /// Explicit skill collection and scope limits.
    #[serde(default)]
    pub skills_limits: Vec<String>,
    /// Immutable collector revision. Missing means no snapshot was collected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_revision: Option<String>,
}

/// Independent current scopes. Neither scope establishes effective agent skills.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentInventories {
    /// Current global configured definitions.
    #[serde(default)]
    pub global: InventorySnapshot,
    /// Current repository configured definitions.
    #[serde(default)]
    pub repo: InventorySnapshot,
}

impl CurrentInventories {
    pub(crate) fn merge_into(&self, context: &mut serde_json::Value) {
        let snapshots = serde_json::to_value(self).expect("inventory records serialize");
        for scope in ["global", "repo"] {
            let root = context
                .as_object_mut()
                .expect("trigger context is an object")
                .entry(scope)
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .expect("inventory scope is an object");
            for field in [
                "instructions",
                "instructions_complete",
                "instructions_limits",
                "skills",
                "skills_complete",
                "skills_limits",
                "snapshot_revision",
            ] {
                root.remove(field);
            }
            root.extend(
                snapshots[scope]
                    .as_object()
                    .expect("inventory snapshot is an object")
                    .clone(),
            );
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}
