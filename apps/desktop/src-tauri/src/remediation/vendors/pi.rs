use std::path::Path;

use antiburn_local::analysis::{ModelControlObservation, SourceFormat};
use antiburn_local::model::AgentKind;

use super::{ActionSupport, RemediationAction, VendorRemediationPolicy, routed_setting_observed};
use crate::agent_config::ConfigSetting;

pub(super) static POLICY: PiPolicy = PiPolicy;

pub(super) struct PiPolicy;

fn trusted_workspace(candidate: Option<&Path>, root: Option<&Path>) -> bool {
    match (candidate, root) {
        (None, None) => true,
        (Some(candidate), Some(root)) => candidate
            .canonicalize()
            .is_ok_and(|candidate| candidate.starts_with(root)),
        _ => false,
    }
}

impl VendorRemediationPolicy for PiPolicy {
    fn agent(&self) -> AgentKind {
        AgentKind::Pi
    }

    fn action_support(&self, action: RemediationAction, source: SourceFormat) -> ActionSupport {
        if source == SourceFormat::PiV3Jsonl
            && matches!(
                action,
                RemediationAction::AutomaticEdit(_)
                    | RemediationAction::RecoverUncertainWrite(_)
                    | RemediationAction::PublicationAttribution(_)
            )
        {
            ActionSupport::Supported
        } else {
            ActionSupport::Unsupported
        }
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

    fn workspace_precedence_supported(
        &self,
        workspace_candidate: Option<&Path>,
        trusted_root: Option<&Path>,
    ) -> bool {
        trusted_workspace(workspace_candidate, trusted_root)
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
