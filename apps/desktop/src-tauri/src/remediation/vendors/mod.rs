use std::path::Path;

use antiburn_local::analysis::{ModelControlObservation, SourceFormat};
use antiburn_local::model::AgentKind;
use antiburn_local::model_catalog::fixed_route_target;

use crate::agent_config::ConfigSetting;

mod antigravity;
mod claude;
mod codex;
mod opencode;
mod pi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RemediationAction {
    AutomaticEdit(ConfigSetting),
    RecoverUncertainWrite(ConfigSetting),
    PublicationAttribution(ConfigSetting),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActionSupport {
    Supported,
    Unsupported,
}

pub(super) trait VendorRemediationPolicy: Sync {
    fn agent(&self) -> AgentKind;

    fn action_support(&self, action: RemediationAction, source: SourceFormat) -> ActionSupport;

    fn runtime_override_present(&self) -> bool {
        false
    }

    fn managed_configuration_present(&self, _home: &Path) -> bool {
        false
    }

    fn workspace_precedence_supported(
        &self,
        _workspace_candidate: Option<&Path>,
        _trusted_root: Option<&Path>,
    ) -> bool {
        true
    }

    fn publication_setting_observed(
        &self,
        _setting: ConfigSetting,
        _value: &str,
        _effective_model: Option<&str>,
        _observations: &[ModelControlObservation],
    ) -> bool {
        false
    }
}

pub(super) fn vendor_policy(agent: AgentKind) -> Option<&'static dyn VendorRemediationPolicy> {
    match agent {
        AgentKind::Claude => Some(&claude::POLICY),
        AgentKind::Codex => Some(&codex::POLICY),
        AgentKind::OpenCode => Some(&opencode::POLICY),
        AgentKind::Pi => Some(&pi::POLICY),
        AgentKind::Antigravity => Some(&antigravity::POLICY),
        AgentKind::Cursor
        | AgentKind::Copilot
        | AgentKind::Cline
        | AgentKind::Kiro
        | AgentKind::AmpCode
        | AgentKind::Windsurf => None,
    }
}

pub(super) fn fixed_route_setting_observed(
    agent: AgentKind,
    setting: ConfigSetting,
    value: &str,
    effective_model: Option<&str>,
    observations: &[ModelControlObservation],
) -> bool {
    let model = match setting {
        ConfigSetting::Model => value,
        ConfigSetting::Reasoning => effective_model.unwrap_or_default(),
    };
    let Some(route) = fixed_route_target(agent.slug(), model) else {
        return false;
    };
    let mut relevant = observations
        .iter()
        .filter(|observation| observation.turns.main_loop > 0)
        .peekable();
    relevant.peek().is_some()
        && relevant.all(|observation| {
            observation.provider.as_deref() == Some(route.provider.as_str())
                && observation.api.as_deref() == Some(route.api.as_str())
                && observation.model == model
                && (setting == ConfigSetting::Model || observation.effort.as_deref() == Some(value))
        })
}

pub(super) fn routed_setting_observed(
    setting: ConfigSetting,
    value: &str,
    effective_model: Option<&str>,
    observations: &[ModelControlObservation],
) -> bool {
    let route = match setting {
        ConfigSetting::Model => value,
        ConfigSetting::Reasoning => effective_model.unwrap_or_default(),
    };
    let Some((provider, model)) = route.split_once('/') else {
        return false;
    };
    let mut relevant = observations
        .iter()
        .filter(|observation| observation.turns.main_loop > 0)
        .peekable();
    relevant.peek().is_some()
        && relevant.all(|observation| {
            observation.provider.as_deref() == Some(provider)
                && observation.model == model
                && (setting == ConfigSetting::Model || observation.effort.as_deref() == Some(value))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use antiburn_local::analysis::TurnCounts;

    const SOURCE_FORMATS: [SourceFormat; 26] = [
        SourceFormat::ClaudeJsonl,
        SourceFormat::CodexRolloutJsonl,
        SourceFormat::OpenCodeJsonl,
        SourceFormat::OpenCodeSqliteV2,
        SourceFormat::PiV3Jsonl,
        SourceFormat::CursorJsonl,
        SourceFormat::CursorCliAgentJsonl,
        SourceFormat::CursorCliStoreDb,
        SourceFormat::CursorIdeComposer,
        SourceFormat::CursorLegacyChatJson,
        SourceFormat::AntigravityJson,
        SourceFormat::AntigravityBrainJsonl,
        SourceFormat::AntigravityCascadeJson,
        SourceFormat::AntigravityWorkspaceChatJson,
        SourceFormat::AntigravitySqlite,
        SourceFormat::CopilotCliJsonl,
        SourceFormat::CopilotIdeChatJson,
        SourceFormat::ClineSessionJson,
        SourceFormat::KiroSessionJson,
        SourceFormat::KiroChat,
        SourceFormat::AmpThreadJson,
        SourceFormat::AmpFileChanges,
        SourceFormat::WindsurfWorkspaceJson,
        SourceFormat::WindsurfMirrorJson,
        SourceFormat::WindsurfCascadeProtobuf,
        SourceFormat::Uncharacterized,
    ];

    #[test]
    fn vendor_action_matrix_covers_all_agents_sources_and_actions() {
        let actions = [
            RemediationAction::AutomaticEdit(ConfigSetting::Model),
            RemediationAction::AutomaticEdit(ConfigSetting::Reasoning),
            RemediationAction::RecoverUncertainWrite(ConfigSetting::Model),
            RemediationAction::RecoverUncertainWrite(ConfigSetting::Reasoning),
            RemediationAction::PublicationAttribution(ConfigSetting::Model),
            RemediationAction::PublicationAttribution(ConfigSetting::Reasoning),
        ];
        let mut supported = Vec::new();
        for &agent in AgentKind::ALL {
            for &source in &SOURCE_FORMATS {
                for &action in &actions {
                    let support =
                        vendor_policy(agent).map_or(ActionSupport::Unsupported, |policy| {
                            assert_eq!(policy.agent(), agent);
                            policy.action_support(action, source)
                        });
                    if support == ActionSupport::Supported {
                        supported.push((agent, source, action));
                    }
                }
            }
        }
        assert_eq!(supported.len(), 24);
    }

    #[test]
    fn unsupported_vendor_modules_do_not_claim_actions() {
        let cases = [
            (AgentKind::OpenCode, SourceFormat::OpenCodeJsonl),
            (AgentKind::Pi, SourceFormat::PiV3Jsonl),
            (AgentKind::Antigravity, SourceFormat::AntigravityJson),
        ];
        for (agent, source) in cases {
            let policy = vendor_policy(agent).unwrap();
            assert_eq!(
                policy.action_support(
                    RemediationAction::AutomaticEdit(ConfigSetting::Reasoning),
                    source
                ),
                if agent == AgentKind::Pi {
                    ActionSupport::Supported
                } else {
                    ActionSupport::Unsupported
                }
            );
        }
    }

    #[test]
    fn complete_controls_must_match_each_effective_typed_setting() {
        let observation = |provider: &str, api: &str, model: &str, effort: Option<&str>| {
            ModelControlObservation {
                provider: Some(provider.to_owned()),
                api: Some(api.to_owned()),
                model: model.to_owned(),
                effort: effort.map(str::to_owned),
                speed: None,
                last_ts_ms: 100,
                turns: TurnCounts {
                    main_loop: 1,
                    delegated: 0,
                },
            }
        };
        let cases = [
            (
                AgentKind::Claude,
                "claude-opus-5",
                observation("anthropic", "messages", "claude-opus-5", Some("max")),
            ),
            (
                AgentKind::Codex,
                "gpt-5.6-sol",
                observation("openai", "responses", "gpt-5.6-sol", Some("xhigh")),
            ),
            (
                AgentKind::OpenCode,
                "openai/gpt-5.6-sol",
                observation("openai", "responses", "gpt-5.6-sol", None),
            ),
            (
                AgentKind::Pi,
                "openai/gpt-5.6",
                observation("openai", "openai-responses", "gpt-5.6", Some("max")),
            ),
        ];
        for (agent, effective_model, observed) in cases {
            let policy = vendor_policy(agent).unwrap();
            assert!(policy.publication_setting_observed(
                ConfigSetting::Model,
                effective_model,
                Some(effective_model),
                std::slice::from_ref(&observed),
            ));
            if agent != AgentKind::OpenCode {
                assert!(policy.publication_setting_observed(
                    ConfigSetting::Reasoning,
                    observed.effort.as_deref().unwrap(),
                    Some(effective_model),
                    std::slice::from_ref(&observed),
                ));
            }
            let mut conflicting = observed.clone();
            conflicting.model.push_str("-other");
            assert!(!policy.publication_setting_observed(
                ConfigSetting::Model,
                effective_model,
                Some(effective_model),
                &[observed, conflicting],
            ));
        }
    }
}
