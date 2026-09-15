mod antigravity;
mod claude;
mod codex;
mod json;
mod opencode;
mod pi;

use std::path::{Path, PathBuf};

use antiburn_local::model::AgentKind;

use super::{ConfigScope, ConfigSetting, ConfigUnavailableReason};

use antigravity::ANTIGRAVITY;
use claude::CLAUDE;
use codex::CODEX;
use opencode::OPENCODE;
use pi::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VendorPolicy {
    AutomaticEdit,
    Unsupported(ConfigUnavailableReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OperationSelector {
    JsonKey(&'static str),
    TomlKey(&'static str),
    PiModel,
    PiModelThinkingLevel(String),
    ClaudeModelEffort(String),
}

impl OperationSelector {
    pub(super) fn physical_selector(&self) -> &'static str {
        match self {
            Self::JsonKey("model") => "model",
            Self::JsonKey("effortLevel") => "effortLevel",
            Self::JsonKey("defaultThinkingLevel") => "defaultThinkingLevel",
            Self::JsonKey(_) => "json-key",
            Self::TomlKey("model") => "model",
            Self::TomlKey("model_reasoning_effort") => "model_reasoning_effort",
            Self::TomlKey(_) => "toml-key",
            Self::PiModel => "defaultProvider+defaultModel",
            Self::PiModelThinkingLevel(_) => "modelThinkingLevels",
            Self::ClaudeModelEffort(_) => "modelSettings.effortLevel",
        }
    }
}

pub(super) struct Target {
    pub(super) path: PathBuf,
    pub(super) safety_root: PathBuf,
    pub(super) scope: ConfigScope,
    pub(super) operation: OperationSelector,
}

pub(super) trait VendorConfig: Sync {
    fn policy(&self, setting: ConfigSetting) -> VendorPolicy;

    fn resolve_target(
        &self,
        setting: ConfigSetting,
        _home: &Path,
        _workspace_cwd: Option<&Path>,
        _trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        Err(unavailable_reason(self.policy(setting)))
    }

    fn read_value(
        &self,
        _bytes: &[u8],
        _operation: &OperationSelector,
    ) -> Result<Option<String>, ConfigUnavailableReason> {
        Err(ConfigUnavailableReason::UnsupportedSetting)
    }

    #[cfg(not(windows))]
    fn edit_value(
        &self,
        _bytes: &[u8],
        _operation: &OperationSelector,
        _proposed: &str,
    ) -> Result<Vec<u8>, ConfigUnavailableReason> {
        Err(ConfigUnavailableReason::UnsupportedSetting)
    }
}

fn unavailable_reason(policy: VendorPolicy) -> ConfigUnavailableReason {
    match policy {
        VendorPolicy::Unsupported(reason) => reason,
        VendorPolicy::AutomaticEdit => ConfigUnavailableReason::UnsupportedSetting,
    }
}

struct UnsupportedVendor;

impl VendorConfig for UnsupportedVendor {
    fn policy(&self, _: ConfigSetting) -> VendorPolicy {
        VendorPolicy::Unsupported(ConfigUnavailableReason::UnsupportedAgent)
    }
}

static UNSUPPORTED: UnsupportedVendor = UnsupportedVendor;

pub(super) fn vendor_for(agent: AgentKind) -> &'static dyn VendorConfig {
    match agent {
        AgentKind::Claude => &CLAUDE,
        AgentKind::Codex => &CODEX,
        AgentKind::OpenCode => &OPENCODE,
        AgentKind::Pi => &PI,
        AgentKind::Antigravity => &ANTIGRAVITY,
        _ => &UNSUPPORTED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_setting_support_is_table_driven() {
        let cases = [
            (AgentKind::Claude, ConfigSetting::Model, true),
            (AgentKind::Claude, ConfigSetting::Reasoning, true),
            (AgentKind::Codex, ConfigSetting::Model, true),
            (AgentKind::Codex, ConfigSetting::Reasoning, true),
            (AgentKind::OpenCode, ConfigSetting::Model, true),
            (AgentKind::OpenCode, ConfigSetting::Reasoning, false),
            (AgentKind::Pi, ConfigSetting::Model, true),
            (AgentKind::Pi, ConfigSetting::Reasoning, true),
            (AgentKind::Antigravity, ConfigSetting::Model, false),
            (AgentKind::Antigravity, ConfigSetting::Reasoning, false),
        ];
        for (agent, setting, supported) in cases {
            assert_eq!(
                vendor_for(agent).policy(setting) == VendorPolicy::AutomaticEdit,
                supported,
                "{agent:?} {setting:?}"
            );
        }
    }
}
