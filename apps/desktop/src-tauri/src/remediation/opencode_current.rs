use super::*;
use crate::agent_config::opencode_config_authority::CurrentConfigAuthority;

pub(crate) struct AuthorityCache(
    crate::agent_config::opencode_config_authority::CurrentConfigAuthorities,
);

impl AuthorityCache {
    pub(crate) fn new() -> Self {
        Self(crate::agent_config::opencode_config_authority::CurrentConfigAuthorities::for_worker())
    }

    pub(super) fn for_listing() -> Self {
        Self(crate::agent_config::opencode_config_authority::CurrentConfigAuthorities::default())
    }

    pub(super) fn get(&mut self, cwd: &Path) -> Option<&CurrentConfigAuthority> {
        self.0.capture(cwd)
    }

    pub(crate) async fn get_async(&mut self, cwd: &Path) -> Option<&CurrentConfigAuthority> {
        self.0.capture_async(cwd).await
    }

    pub(super) fn get_for_recovery(
        &mut self,
        cwd: &Path,
    ) -> Result<
        &CurrentConfigAuthority,
        crate::agent_config::opencode_config_authority::CaptureUnavailable,
    > {
        self.0.try_capture(cwd)
    }
}

pub(super) fn resource_location(
    store: &Store,
    resource: &insights_report::UnusedResourceTarget,
) -> Option<PathBuf> {
    let locations = resource
        .supporting_sessions
        .iter()
        .map(|session| {
            store
                .session(&SessionKey::new(
                    &session.environment_key,
                    &session.agent,
                    &session.session_id,
                ))
                .ok()
                .flatten()?
                .cwd
                .map(PathBuf::from)
        })
        .collect::<Option<BTreeSet<_>>>()?;
    (locations.len() == 1)
        .then(|| locations.into_iter().next())
        .flatten()
}

pub(super) fn resource_authority_eligible(
    resource: &insights_report::UnusedResourceTarget,
    environment_key: &str,
    home: Option<&Path>,
) -> bool {
    use crate::agent_config::ResourceKind;

    let setting = match resource.kind {
        ResourceKind::McpServer => ConfigSetting::McpServer,
        ResourceKind::Skill => ConfigSetting::Skill,
        ResourceKind::BuiltInTool => return false,
    };
    let scope = match &resource.scope {
        insights_report::ResourceAssessmentScope::Global => "global",
        insights_report::ResourceAssessmentScope::Project(_) => "project",
    };
    resource.agent == AgentKind::OpenCode
        && resource.indexed
        && home.is_some()
        && automatic_editor_supported(
            resource.agent,
            setting,
            resource.source_format,
            scope,
            environment_key,
            current_editor_platform(),
        )
}

pub(super) fn operation_reviewed(
    resolved: &crate::agent_config::opencode_current::OpenCodeResolvedTarget,
    operation: &ConfigOperation,
) -> bool {
    resolved.matches_operation(operation)
        && match operation.setting {
            ConfigSetting::Model => resolved.reviewed_default_route(),
            ConfigSetting::SubagentModel => resolved.reviewed_model_route(),
            _ => true,
        }
        && (!matches!(
            operation.setting,
            ConfigSetting::Model | ConfigSetting::SubagentModel
        ) || opencode_proposed_value(operation)
            .is_some_and(|value| resolved.reviewed_proposed_model_route(&value)))
}

pub(super) struct OpenCodeConfigResolution<'a> {
    pub secret: &'a [u8; 32],
    pub environment_key: &'a str,
    pub finding: &'a Finding,
    pub context: ConfigContext,
    pub authority: &'a CurrentConfigAuthority,
    pub require_scope_match: bool,
}

impl RemediationController {
    pub(super) fn resolve_opencode_config(
        &self,
        store: &Store,
        identity: &mut TargetIdentity,
        request: OpenCodeConfigResolution<'_>,
    ) -> Result<Option<CachedConfig>, ControllerError> {
        let OpenCodeConfigResolution {
            secret,
            environment_key,
            finding,
            context,
            authority,
            require_scope_match,
        } = request;
        let Some(subject) = opencode_subject(finding.cause()) else {
            return Ok(None);
        };
        let context = refreshed_config_context(&context);
        let Ok(resolved) = self.editor.resolve_opencode(&context, authority, subject) else {
            return Ok(None);
        };
        let operation =
            reviewed_config_operation(AgentKind::OpenCode, finding.cause()).or_else(|| {
                resolved
                    .current_value_display()
                    .and_then(|current| compaction_operation(finding.cause(), &current))
            });
        let Some(operation) = operation else {
            return Ok(None);
        };
        if !automatic_editor_supported(
            AgentKind::OpenCode,
            operation.setting,
            finding.source_format,
            scope_name(resolved.scope()),
            environment_key,
            current_editor_platform(),
        ) || !operation_reviewed(&resolved, &operation)
            || (require_scope_match && scope_name(resolved.scope()) != identity.scope_kind)
        {
            return Ok(None);
        }
        let key = physical_key(
            store,
            AgentKind::OpenCode,
            resolved.physical_identity(),
            resolved.selector_qualifier(),
        )
        .map_err(|_| ControllerError::Internal)?;
        match identity.physical_target_key.as_deref() {
            Some(attributed)
                if attributed != key || identity.scope_kind != scope_name(resolved.scope()) =>
            {
                return Ok(None);
            }
            None if operation.setting == ConfigSetting::Model => return Ok(None),
            None => bind_current_config_identity(
                identity,
                secret,
                environment_key,
                finding,
                AgentKind::OpenCode,
                resolved.scope(),
                &key,
            ),
            _ => {}
        }
        let display_path = display_config_file(resolved.physical_path(), &context.home_root);
        Ok(Some(CachedConfig {
            context,
            additional_contexts: Vec::new(),
            operation,
            physical_key: key,
            display_path,
        }))
    }

    pub(super) fn resolve_opencode_target(
        &self,
        store: &Store,
        finding: CurrentFinding,
        home: Option<&Path>,
        authority: Option<&CurrentConfigAuthority>,
    ) -> Result<(String, CachedTarget), ControllerError> {
        let root = finding
            .workspace_candidate()
            .and_then(|cwd| trusted_workspace(store, cwd).ok().flatten());
        let secret = store
            .provider_account_secret()
            .map_err(|_| ControllerError::Internal)?;
        let mut identity = target_identity(&secret, &finding, root.as_deref());
        let mut config = None;
        if let (Some(home), Some(cwd), Some(root), Some(authority)) = (
            home,
            finding.workspace_candidate(),
            root.as_deref(),
            authority,
        ) {
            let mut context = ConfigContext::native_workspace(AgentKind::OpenCode, home, cwd, root);
            context.source_format = Some(finding.finding.source_format);
            config = self.resolve_opencode_config(
                store,
                &mut identity,
                OpenCodeConfigResolution {
                    secret: &secret,
                    environment_key: &finding.environment_key,
                    finding: &finding.finding,
                    context,
                    authority,
                    require_scope_match: false,
                },
            )?;
        }
        Ok((
            identity.group_key,
            CachedTarget {
                findings: vec![finding],
                resource: None,
                target_key: identity.target_key,
                canonical_identity: identity.canonical_identity,
                workspace_key: identity.workspace_key,
                agent: AgentKind::OpenCode,
                scope_kind: identity.scope_kind,
                scope_key: identity.scope_key,
                physical_target_key: identity.physical_target_key,
                config,
            },
        ))
    }
}
