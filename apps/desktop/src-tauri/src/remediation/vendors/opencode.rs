use std::path::Path;

use antiburn_local::analysis::{ModelControlObservation, SourceFormat};
use antiburn_local::model::AgentKind;

use super::{ActionSupport, RemediationAction, VendorRemediationPolicy, routed_setting_observed};
use crate::agent_config::ConfigSetting;

pub(super) static POLICY: OpenCodePolicy = OpenCodePolicy;

pub(super) struct OpenCodePolicy;

const RUNTIME_OVERRIDE_NAMES: [&str; 6] = [
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_DIR",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_AUTH_CONTENT",
    "OPENCODE_DB",
    "OPENCODE_DATA_DIR",
];

impl VendorRemediationPolicy for OpenCodePolicy {
    fn agent(&self) -> AgentKind {
        AgentKind::OpenCode
    }

    fn action_support(&self, action: RemediationAction, source: SourceFormat) -> ActionSupport {
        if matches!(
            source,
            SourceFormat::OpenCodeJsonl | SourceFormat::OpenCodeSqliteV2
        ) && matches!(
            action,
            RemediationAction::AutomaticEdit(ConfigSetting::Model)
                | RemediationAction::RecoverUncertainWrite(ConfigSetting::Model)
                | RemediationAction::PublicationAttribution(ConfigSetting::Model)
        ) {
            ActionSupport::Supported
        } else {
            ActionSupport::Unsupported
        }
    }

    fn managed_configuration_present(&self, _home: &Path) -> bool {
        #[cfg(target_os = "macos")]
        let root = Path::new("/Library/Application Support/opencode");
        #[cfg(all(unix, not(target_os = "macos")))]
        let root = Path::new("/etc/opencode");
        #[cfg(windows)]
        return false;
        #[cfg(not(windows))]
        return [root.join("opencode.json"), root.join("opencode.jsonc")]
            .iter()
            .any(|path| path.try_exists().unwrap_or(true));
    }

    fn runtime_override_present(&self) -> bool {
        RUNTIME_OVERRIDE_NAMES
            .iter()
            .any(|name| std::env::var_os(name).is_some())
    }

    fn workspace_precedence_supported(
        &self,
        workspace_candidate: Option<&Path>,
        trusted_root: Option<&Path>,
    ) -> bool {
        trusted_workspace(workspace_candidate, trusted_root)
    }

    fn publication_setting_observed(
        &self,
        setting: ConfigSetting,
        value: &str,
        effective_model: Option<&str>,
        observations: &[ModelControlObservation],
    ) -> bool {
        routed_setting_observed(setting, value, effective_model, observations)
    }
}

fn trusted_workspace(candidate: Option<&Path>, root: Option<&Path>) -> bool {
    match (candidate, root) {
        (None, None) => true,
        (Some(candidate), Some(root)) => candidate
            .canonicalize()
            .is_ok_and(|candidate| candidate.starts_with(root)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cwd_sensitive_actions_allow_safe_nested_workspaces() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let nested = root.join("nested");
        std::fs::create_dir(&nested).unwrap();
        let cases = [
            (None, None, true),
            (Some(root.as_path()), Some(root.as_path()), true),
            (Some(nested.as_path()), Some(root.as_path()), true),
            (Some(root.as_path()), None, false),
        ];
        for (candidate, trusted, expected) in cases {
            assert_eq!(trusted_workspace(candidate, trusted), expected);
        }
    }
}
