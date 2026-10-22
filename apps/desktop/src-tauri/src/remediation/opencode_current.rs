use super::*;
use crate::agent_config::opencode_config_authority::CurrentConfigAuthority;

pub(super) struct AuthorityCache {
    started: std::time::Instant,
    receipts: BTreeMap<PathBuf, Option<CurrentConfigAuthority>>,
    bytes: usize,
}

impl AuthorityCache {
    pub(super) fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            receipts: BTreeMap::new(),
            bytes: 0,
        }
    }

    pub(super) fn get(&mut self, cwd: &Path) -> Option<&CurrentConfigAuthority> {
        if !self.receipts.contains_key(cwd) {
            if self.receipts.len() >= 8
                || self.started.elapsed() > std::time::Duration::from_secs(3)
            {
                return None;
            }
            #[cfg(not(test))]
            let receipt = CurrentConfigAuthority::registered(cwd).ok();
            #[cfg(test)]
            let receipt: Option<CurrentConfigAuthority> = None;
            let bytes = receipt
                .as_ref()
                .map_or(0, CurrentConfigAuthority::retained_bytes);
            if self.bytes.saturating_add(bytes) > PREPARED_CACHE_BYTES {
                return None;
            }
            self.bytes = self.bytes.saturating_add(bytes);
            self.receipts.insert(cwd.into(), receipt);
        }
        self.receipts.get(cwd).and_then(Option::as_ref)
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

impl RemediationController {
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
        if let (Some(home), Some(cwd), Some(root), Some(authority), Some(subject)) = (
            home,
            finding.workspace_candidate(),
            root.as_deref(),
            authority,
            opencode_subject(finding.finding.cause()),
        ) {
            let mut context = ConfigContext::native_workspace(AgentKind::OpenCode, home, cwd, root);
            context.source_format = Some(finding.finding.source_format);
            if let Ok(resolved) = self.editor.resolve_opencode(&context, authority, subject) {
                let operation =
                    reviewed_config_operation(AgentKind::OpenCode, finding.finding.cause())
                        .or_else(|| {
                            resolved.current_value_display().and_then(|current| {
                                compaction_operation(finding.finding.cause(), &current)
                            })
                        });
                if let Some(operation) = operation
                    && automatic_editor_supported(
                        AgentKind::OpenCode,
                        operation.setting,
                        finding.finding.source_format,
                        scope_name(resolved.scope()),
                        &finding.environment_key,
                        current_editor_platform(),
                    )
                    && resolved.matches_operation(&operation)
                    && (operation.setting != ConfigSetting::Model
                        || resolved.reviewed_default_route())
                    && (operation.setting != ConfigSetting::SubagentModel
                        || resolved.reviewed_model_route())
                    && (!matches!(
                        operation.setting,
                        ConfigSetting::Model | ConfigSetting::SubagentModel
                    ) || opencode_proposed_value(&operation)
                        .is_some_and(|value| resolved.reviewed_proposed_model_route(&value)))
                {
                    let key = physical_key(
                        store,
                        AgentKind::OpenCode,
                        resolved.physical_identity(),
                        resolved.selector_qualifier(),
                    )
                    .map_err(|_| ControllerError::Internal)?;
                    let attributed = identity.physical_target_key.as_deref();
                    if (operation.setting != ConfigSetting::Model && attributed.is_none())
                        || (attributed == Some(key.as_str())
                            && identity.scope_kind == scope_name(resolved.scope()))
                    {
                        if attributed.is_none() {
                            bind_current_config_identity(
                                &mut identity,
                                &secret,
                                &finding.environment_key,
                                &finding.finding,
                                AgentKind::OpenCode,
                                resolved.scope(),
                                &key,
                            );
                        }
                        config = Some(CachedConfig {
                            context,
                            additional_contexts: Vec::new(),
                            operation,
                            physical_key: key,
                            display_path: display_config_file(resolved.physical_path(), home),
                        });
                    }
                }
            }
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
