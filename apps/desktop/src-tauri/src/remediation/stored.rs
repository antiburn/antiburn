use antiburn_local::analysis::SourceFormat;
use antiburn_local::pricing::ModelPricing;
use anyhow::Result;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{BurnCheckDisplayFacts, SavingsStatus, VerificationStatus};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WatchDefinition {
    pub version: u32,
    pub detector: String,
    pub canonical_identity: String,
    pub source_format: StoredSourceFormat,
    pub workspace_key: Option<String>,
    #[serde(default)]
    pub workspace_relative_cwd: Option<String>,
    pub provider: Option<String>,
    pub api: Option<String>,
    pub old_model: Option<String>,
    pub replacement: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
    pub physical_target_key: Option<String>,
    #[serde(default)]
    pub config_setting: Option<String>,
    #[serde(default)]
    pub config_expected_value: Option<String>,
    #[serde(default)]
    pub config_proposed_value: Option<String>,
    #[serde(default)]
    pub config_original_bytes_hash: Option<String>,
    #[serde(default)]
    pub config_proposed_bytes_hash: Option<String>,
    pub verification_method_revision: u32,
    #[serde(default)]
    pub remediation_policy_revision: Option<u32>,
    pub savings_method_revision: u32,
    pub pricing_revision: Option<String>,
    pub old_pricing: Option<ModelPricing>,
    pub replacement_pricing: Option<ModelPricing>,
    #[serde(default)]
    pub catalog_revision: Option<i64>,
    #[serde(default)]
    pub target_model: Option<String>,
    #[serde(default)]
    pub target_control: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoredSourceFormat(SourceFormat);

impl StoredSourceFormat {
    pub(super) const fn value(self) -> SourceFormat {
        self.0
    }
}

impl From<SourceFormat> for StoredSourceFormat {
    fn from(value: SourceFormat) -> Self {
        Self(value)
    }
}

impl From<&str> for StoredSourceFormat {
    fn from(value: &str) -> Self {
        Self(legacy_source_format(value).expect("the test fixture uses a known source format"))
    }
}

impl Serialize for StoredSourceFormat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StoredSourceFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_source_format(deserializer).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StoredDisplaySnapshot {
    pub version: u32,
    pub finding_id: String,
    pub display: BurnCheckDisplayFacts,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredResult {
    pub version: u32,
    pub verification: VerificationStatus,
    pub savings: SavingsStatus,
    #[serde(default)]
    pub observed_at_ms: Option<i64>,
}

pub(super) fn stored_result(value: &str) -> Result<StoredResult> {
    let result: StoredResult = serde_json::from_str(value)?;
    anyhow::ensure!(
        result.version == 1,
        "unsupported remediation result version"
    );
    Ok(result)
}

pub(super) fn validate_envelope_version(value: &str, name: &str) -> Result<()> {
    let envelope: serde_json::Value = serde_json::from_str(value)?;
    anyhow::ensure!(
        envelope.get("version").and_then(serde_json::Value::as_u64) == Some(1),
        "unsupported {name} version"
    );
    Ok(())
}

pub(super) fn parse_watch_definition(value: &str) -> Result<WatchDefinition> {
    let definition: WatchDefinition = serde_json::from_str(value)?;
    anyhow::ensure!(
        definition.version == 1,
        "unsupported remediation definition version"
    );
    Ok(definition)
}

pub(super) fn parse_display_snapshot(value: &str) -> Result<StoredDisplaySnapshot> {
    let snapshot: StoredDisplaySnapshot = serde_json::from_str(value)?;
    anyhow::ensure!(
        snapshot.version == 1,
        "unsupported remediation display version"
    );
    Ok(snapshot)
}

fn deserialize_source_format<'de, D>(deserializer: D) -> Result<SourceFormat, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if let Ok(source_format) = serde_json::from_value(serde_json::Value::String(value.clone())) {
        return Ok(source_format);
    }
    legacy_source_format(&value)
        .ok_or_else(|| serde::de::Error::custom(format!("unknown source format `{value}`")))
}

fn legacy_source_format(value: &str) -> Option<SourceFormat> {
    Some(match value {
        "ClaudeJsonl" => SourceFormat::ClaudeJsonl,
        "CodexRolloutJsonl" => SourceFormat::CodexRolloutJsonl,
        "OpenCodeJsonl" => SourceFormat::OpenCodeJsonl,
        "OpenCodeSqliteV2" => SourceFormat::OpenCodeSqliteV2,
        "PiV3Jsonl" => SourceFormat::PiV3Jsonl,
        "CursorJsonl" => SourceFormat::CursorJsonl,
        "CursorCliAgentJsonl" => SourceFormat::CursorCliAgentJsonl,
        "CursorCliStoreDb" => SourceFormat::CursorCliStoreDb,
        "CursorIdeComposer" => SourceFormat::CursorIdeComposer,
        "CursorLegacyChatJson" => SourceFormat::CursorLegacyChatJson,
        "AntigravityJson" => SourceFormat::AntigravityJson,
        "AntigravityBrainJsonl" => SourceFormat::AntigravityBrainJsonl,
        "AntigravityCascadeJson" => SourceFormat::AntigravityCascadeJson,
        "AntigravityWorkspaceChatJson" => SourceFormat::AntigravityWorkspaceChatJson,
        "AntigravitySqlite" => SourceFormat::AntigravitySqlite,
        "CopilotCliJsonl" => SourceFormat::CopilotCliJsonl,
        "CopilotIdeChatJson" => SourceFormat::CopilotIdeChatJson,
        "ClineSessionJson" => SourceFormat::ClineSessionJson,
        "KiroSessionJson" => SourceFormat::KiroSessionJson,
        "KiroChat" => SourceFormat::KiroChat,
        "AmpThreadJson" => SourceFormat::AmpThreadJson,
        "AmpFileChanges" => SourceFormat::AmpFileChanges,
        "WindsurfWorkspaceJson" => SourceFormat::WindsurfWorkspaceJson,
        "WindsurfMirrorJson" => SourceFormat::WindsurfMirrorJson,
        "WindsurfCascadeProtobuf" => SourceFormat::WindsurfCascadeProtobuf,
        "Uncharacterized" => SourceFormat::Uncharacterized,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_definition_accepts_legacy_and_stable_source_formats() {
        let definition = |source_format: &str| {
            format!(
                r#"{{"version":1,"detector":"unused_skills","canonicalIdentity":"target","sourceFormat":"{source_format}","workspaceKey":null,"provider":null,"api":null,"oldModel":null,"replacement":null,"physicalTargetKey":null,"verificationMethodRevision":1,"savingsMethodRevision":1,"pricingRevision":null,"oldPricing":null,"replacementPricing":null}}"#
            )
        };
        assert_eq!(
            parse_watch_definition(&definition("ClaudeJsonl"))
                .unwrap()
                .source_format,
            StoredSourceFormat::from(SourceFormat::ClaudeJsonl)
        );
        let stable = parse_watch_definition(&definition("claude_jsonl")).unwrap();
        assert_eq!(stable.source_format.value(), SourceFormat::ClaudeJsonl);
        assert!(
            serde_json::to_string(&stable)
                .unwrap()
                .contains(r#""sourceFormat":"claude_jsonl""#)
        );
    }
}
