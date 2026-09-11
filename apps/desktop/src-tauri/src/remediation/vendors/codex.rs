use std::path::{Path, PathBuf};

use antiburn_local::analysis::{ModelControlObservation, SourceFormat};
use antiburn_local::model::AgentKind;

use super::{
    ActionSupport, RemediationAction, VendorRemediationPolicy, fixed_route_setting_observed,
};
use crate::agent_config::ConfigSetting;

pub(super) static POLICY: CodexPolicy = CodexPolicy;

pub(super) struct CodexPolicy;

impl VendorRemediationPolicy for CodexPolicy {
    fn agent(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn action_support(&self, action: RemediationAction, source: SourceFormat) -> ActionSupport {
        match (action, source) {
            (
                RemediationAction::AutomaticEdit(_)
                | RemediationAction::RecoverUncertainWrite(_)
                | RemediationAction::PublicationAttribution(_),
                SourceFormat::CodexRolloutJsonl,
            ) => ActionSupport::Supported,
            _ => ActionSupport::Unsupported,
        }
    }

    fn runtime_override_present(&self) -> bool {
        ["CODEX_HOME", "CODEX_MODEL", "OPENAI_MODEL"]
            .iter()
            .any(|name| std::env::var_os(name).is_some())
    }

    fn managed_configuration_present(&self, home: &Path) -> bool {
        [
            home.join(".codex/managed_config.toml"),
            home.join(".codex/requirements.toml"),
            PathBuf::from("/etc/codex/config.toml"),
            PathBuf::from("/etc/codex/requirements.toml"),
            PathBuf::from("/etc/codex/managed_config.toml"),
        ]
        .iter()
        .any(|path| path.try_exists().unwrap_or(true))
    }

    fn workspace_precedence_supported(
        &self,
        workspace_candidate: Option<&Path>,
        trusted_root: Option<&Path>,
    ) -> bool {
        match (workspace_candidate, trusted_root) {
            (None, None) => true,
            (Some(candidate), Some(root)) => candidate
                .canonicalize()
                .is_ok_and(|candidate| candidate == root),
            _ => false,
        }
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
    use super::*;

    #[test]
    fn codex_auto_fix_rejects_untrusted_and_nested_working_directories() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let nested = root.join("nested");
        std::fs::create_dir(&nested).unwrap();
        assert!(POLICY.workspace_precedence_supported(Some(&root), Some(&root)));
        assert!(!POLICY.workspace_precedence_supported(Some(&nested), Some(&root)));
        assert!(!POLICY.workspace_precedence_supported(Some(&root), None));
        assert!(
            super::super::claude::POLICY.workspace_precedence_supported(Some(&nested), Some(&root))
        );
    }
}
