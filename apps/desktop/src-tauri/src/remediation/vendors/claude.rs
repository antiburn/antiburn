use std::path::{Path, PathBuf};

use antiburn_local::analysis::{ModelControlObservation, SourceFormat};
use antiburn_local::model::AgentKind;

use super::{
    ActionSupport, RemediationAction, VendorRemediationPolicy, fixed_route_setting_observed,
};
use crate::agent_config::ConfigSetting;

pub(super) static POLICY: ClaudePolicy = ClaudePolicy;

pub(super) struct ClaudePolicy;

impl VendorRemediationPolicy for ClaudePolicy {
    fn agent(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn action_support(&self, action: RemediationAction, source: SourceFormat) -> ActionSupport {
        match (action, source) {
            (
                RemediationAction::AutomaticEdit(_)
                | RemediationAction::RecoverUncertainWrite(_)
                | RemediationAction::PublicationAttribution(_),
                SourceFormat::ClaudeJsonl,
            ) => ActionSupport::Supported,
            _ => ActionSupport::Unsupported,
        }
    }

    fn runtime_override_present(&self) -> bool {
        [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "CLAUDE_CONFIG_DIR",
        ]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
    }

    fn managed_configuration_present(&self, home: &Path) -> bool {
        [
            home.join(".claude/managed-settings.json"),
            PathBuf::from("/etc/claude-code/managed-settings.json"),
            PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json"),
        ]
        .iter()
        .any(|path| path.try_exists().unwrap_or(true))
    }

    fn publication_setting_observed(
        &self,
        setting: ConfigSetting,
        value: &str,
        effective_model: Option<&str>,
        observations: &[ModelControlObservation],
    ) -> bool {
        fixed_route_setting_observed(self.agent(), setting, value, effective_model, observations)
    }
}

#[cfg(test)]
mod tests {
    use antiburn_local::analysis::TurnCounts;

    use super::*;

    #[test]
    fn publication_attribution_requires_the_fixed_route_and_a_main_loop_turn() {
        let observation = |provider: &str, api: &str, main_loop| ModelControlObservation {
            provider: Some(provider.into()),
            api: Some(api.into()),
            model: "claude-opus-4-8".into(),
            effort: None,
            speed: None,
            last_ts_ms: 100,
            turns: TurnCounts {
                main_loop,
                delegated: 1,
            },
        };
        assert!(!POLICY.publication_setting_observed(
            ConfigSetting::Model,
            "claude-opus-4-8",
            None,
            &[observation("gateway", "messages", 1)],
        ));
        assert!(!POLICY.publication_setting_observed(
            ConfigSetting::Model,
            "claude-opus-4-8",
            None,
            &[observation("anthropic", "messages", 0)],
        ));
        assert!(POLICY.publication_setting_observed(
            ConfigSetting::Model,
            "claude-opus-4-8",
            None,
            &[observation("anthropic", "messages", 1)],
        ));
    }
}
