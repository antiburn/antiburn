mod antigravity;
mod claude;
mod codex;
mod cursor;
mod json;
mod opencode;
mod pi;

use std::path::{Path, PathBuf};

use antiburn_local::model::AgentKind;

use super::{ConfigScope, ConfigSetting, ConfigUnavailableReason};

use antigravity::ANTIGRAVITY;
use claude::CLAUDE;
use codex::CODEX;
use cursor::CURSOR;
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
    JsonPath(Vec<&'static str>),
    NamedMarkdownModel(String),
    NamedTomlModel(String),
    NamedTomlMcpServer(String),
    NamedJsonMcpServer(String),
    NamedClaudeMcpServer(String),
    NamedClaudeBuiltInTool(String),
    NamedOpenCodeBuiltInTool(String),
    NamedPiDefaultTool(String),
    NamedClaudeSkill(String),
    NamedTomlSkill(String),
    NamedOpenCodeSkill(String),
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
            Self::JsonPath(path) => match path.as_slice() {
                ["compaction", "auto"] => "compaction.auto",
                ["compaction", "reserved"] => "compaction.reserved",
                ["compaction", "enabled"] => "compaction.enabled",
                ["compaction", "reserveTokens"] => "compaction.reserveTokens",
                ["compaction", "keepRecentTokens"] => "compaction.keepRecentTokens",
                _ => "json-path",
            },
            Self::NamedMarkdownModel(_) => "frontmatter.model",
            Self::NamedTomlModel(_) => "model",
            Self::NamedTomlMcpServer(_) => "mcp_servers.<name>.enabled",
            Self::NamedJsonMcpServer(_) => "mcp.<name>.enabled",
            Self::NamedClaudeMcpServer(_) => "permissions.deny.mcp__<name>__*",
            Self::NamedClaudeBuiltInTool(_) => "permissions.deny.<tool>",
            Self::NamedOpenCodeBuiltInTool(_) => "permissions[].action=<tool>",
            Self::NamedPiDefaultTool(_) => "defaultTools[]",
            Self::NamedClaudeSkill(_) => "skillOverrides.<name>",
            Self::NamedTomlSkill(_) => "skills.config.<name>.enabled",
            Self::NamedOpenCodeSkill(_) => "permissions[].action=skill",
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

    fn resolve_target_for_value(
        &self,
        setting: ConfigSetting,
        expected: Option<&str>,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        let _ = expected;
        self.resolve_target(setting, home, workspace_cwd, trusted_workspace_root)
    }

    #[cfg(not(windows))]
    fn resolve_targets(
        &self,
        setting: ConfigSetting,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Vec<Target>, ConfigUnavailableReason> {
        Ok(vec![self.resolve_target(
            setting,
            home,
            workspace_cwd,
            trusted_workspace_root,
        )?])
    }

    #[cfg(not(windows))]
    fn standalone_global(
        &self,
        _setting: ConfigSetting,
        _home: &Path,
        _proposed: &str,
    ) -> Result<(PathBuf, Vec<u8>), ConfigUnavailableReason> {
        Err(ConfigUnavailableReason::MissingConfig)
    }

    #[cfg(not(windows))]
    fn standalone_selector(&self, _setting: ConfigSetting) -> &'static str {
        "standalone"
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
        AgentKind::Cursor => &CURSOR,
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
            (AgentKind::Cursor, ConfigSetting::Model, true),
            (AgentKind::Antigravity, ConfigSetting::Model, true),
            (AgentKind::Antigravity, ConfigSetting::Reasoning, false),
            (AgentKind::Claude, ConfigSetting::Compaction, true),
            (AgentKind::Codex, ConfigSetting::Compaction, true),
            (AgentKind::Claude, ConfigSetting::FastMode, true),
            (AgentKind::Codex, ConfigSetting::FastMode, true),
            (AgentKind::OpenCode, ConfigSetting::Compaction, true),
            (AgentKind::Pi, ConfigSetting::Compaction, true),
            (AgentKind::Claude, ConfigSetting::SubagentModel, true),
            (AgentKind::Codex, ConfigSetting::SubagentModel, true),
            (AgentKind::OpenCode, ConfigSetting::SubagentModel, true),
            (AgentKind::Claude, ConfigSetting::McpServer, true),
            (AgentKind::Codex, ConfigSetting::McpServer, true),
            (AgentKind::OpenCode, ConfigSetting::McpServer, true),
            (AgentKind::Claude, ConfigSetting::BuiltInTool, true),
            (AgentKind::OpenCode, ConfigSetting::BuiltInTool, true),
            (AgentKind::Pi, ConfigSetting::BuiltInTool, true),
            (AgentKind::Claude, ConfigSetting::Skill, true),
            (AgentKind::Codex, ConfigSetting::Skill, true),
            (AgentKind::OpenCode, ConfigSetting::Skill, true),
        ];
        for (agent, setting, supported) in cases {
            assert_eq!(
                vendor_for(agent).policy(setting) == VendorPolicy::AutomaticEdit,
                supported,
                "{agent:?} {setting:?}"
            );
        }
    }

    #[test]
    fn named_subagent_models_are_available_only_for_reviewed_vendors() {
        for agent in [AgentKind::Claude, AgentKind::Codex, AgentKind::OpenCode] {
            assert_eq!(
                vendor_for(agent).policy(ConfigSetting::SubagentModel),
                VendorPolicy::AutomaticEdit,
                "{agent:?}"
            );
        }
        assert_ne!(
            vendor_for(AgentKind::Pi).policy(ConfigSetting::Skill),
            VendorPolicy::AutomaticEdit
        );
        for agent in [AgentKind::Pi, AgentKind::Cursor, AgentKind::Antigravity] {
            assert_ne!(
                vendor_for(agent).policy(ConfigSetting::SubagentModel),
                VendorPolicy::AutomaticEdit,
                "{agent:?}"
            );
        }
    }
}
