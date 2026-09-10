use super::*;

pub(super) fn trusted_workspace(store: &Store, candidate: &Path) -> Result<Option<PathBuf>> {
    trusted_workspace_in(&store.lock(), candidate)
}

pub(super) fn trusted_workspace_in(
    connection: &rusqlite::Connection,
    candidate: &Path,
) -> Result<Option<PathBuf>> {
    let candidate = candidate.canonicalize()?;
    let mut statement = connection.prepare(
        "SELECT repo_root FROM repository
          WHERE enabled = 1 AND status = 'accessible' AND repo_root IS NOT NULL",
    )?;
    let root = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .filter_map(|root| root.ok())
        .filter_map(|root| PathBuf::from(root).canonicalize().ok())
        .filter(|root| candidate.starts_with(root))
        .max_by_key(|root| root.components().count());
    Ok(root)
}

pub(crate) fn hashed_workspace_key(store: &Store, workspace: &Path) -> Result<String> {
    hashed_value(store, "workspace", &workspace.to_string_lossy())
}

#[derive(Default)]
pub(crate) struct PublicationConfigAttribution {
    pub model: Option<(String, String, String)>,
    pub reasoning: Option<(String, String, String)>,
}

pub(crate) fn publication_config_attribution(
    store: &Store,
    key: &crate::store::SessionKey,
    status: crate::store::PublishedEvidence,
    evidence_json: &str,
) -> Result<PublicationConfigAttribution> {
    let Some(home) = antiburn_local::paths::home_dir() else {
        return Ok(PublicationConfigAttribution::default());
    };
    publication_config_attribution_with_home(store, key, status, evidence_json, &home)
}

pub(crate) fn publication_config_attribution_with_home(
    store: &Store,
    key: &crate::store::SessionKey,
    status: crate::store::PublishedEvidence,
    evidence_json: &str,
    home: &Path,
) -> Result<PublicationConfigAttribution> {
    if status != crate::store::PublishedEvidence::Ready {
        return Ok(PublicationConfigAttribution::default());
    }
    if key.environment_key != "native" {
        return Ok(PublicationConfigAttribution::default());
    }
    let Some(agent) = crate::agents::kind_from_slug(&key.agent) else {
        return Ok(PublicationConfigAttribution::default());
    };
    let Some(policy) = vendor_policy(agent) else {
        return Ok(PublicationConfigAttribution::default());
    };
    let Some(session) = store.session(key)? else {
        return Ok(PublicationConfigAttribution::default());
    };
    let workspace_candidate = session.cwd.as_deref().map(Path::new);
    let workspace =
        workspace_candidate.and_then(|path| trusted_workspace(store, path).ok().flatten());
    if (workspace_candidate.is_some() && workspace.is_none())
        || !workspace_precedence_supported(agent, workspace_candidate, workspace.as_deref())
    {
        return Ok(PublicationConfigAttribution::default());
    }
    let Some(mut context) = config_context(agent, home, workspace_candidate, workspace.as_deref())
    else {
        return Ok(PublicationConfigAttribution::default());
    };
    context.runtime_override_present = runtime_override_present(agent);
    context.managed_configuration_present =
        managed_configuration_present(agent, &context.home_root);
    let editor = AgentConfigEditor::new();
    let effective_model = editor.effective(&context, ConfigSetting::Model).ok();
    let effective_reasoning = editor.effective(&context, ConfigSetting::Reasoning).ok();
    if effective_model.is_none() && effective_reasoning.is_none() {
        return Ok(PublicationConfigAttribution::default());
    }
    let evidence: antiburn_local::analysis::SessionEvidence = serde_json::from_str(evidence_json)?;
    let antiburn_local::analysis::EvidenceValue::Complete(models) = &evidence.models else {
        return Ok(PublicationConfigAttribution::default());
    };
    let model = publication_setting_attribution(
        store,
        policy,
        agent,
        evidence.capabilities.source_format,
        effective_model.as_ref(),
        effective_model.as_ref().map(|value| value.value.as_str()),
        &models.control_observations,
    )?;
    let reasoning = publication_setting_attribution(
        store,
        policy,
        agent,
        evidence.capabilities.source_format,
        effective_reasoning.as_ref(),
        effective_model.as_ref().map(|value| value.value.as_str()),
        &models.control_observations,
    )?;
    Ok(PublicationConfigAttribution { model, reasoning })
}

fn publication_setting_attribution(
    store: &Store,
    policy: &dyn vendors::VendorRemediationPolicy,
    agent: AgentKind,
    source: SourceFormat,
    effective: Option<&crate::agent_config::EffectiveConfig>,
    effective_model: Option<&str>,
    observations: &[antiburn_local::analysis::ModelControlObservation],
) -> Result<Option<(String, String, String)>> {
    let Some(effective) = effective else {
        return Ok(None);
    };
    if policy.action_support(
        RemediationAction::PublicationAttribution(effective.setting),
        source,
    ) == ActionSupport::Unsupported
        || !policy.publication_setting_observed(
            effective.setting,
            &effective.value,
            effective_model,
            observations,
        )
    {
        return Ok(None);
    }
    Ok(Some((
        physical_key(
            store,
            agent,
            effective.physical_identity(),
            physical_selector_value(effective.physical_identity().1, effective_model),
        )?,
        scope_name(effective.scope).to_owned(),
        effective.value.clone(),
    )))
}

pub(super) fn physical_key(
    store: &Store,
    agent: AgentKind,
    (path, semantic): (&Path, &'static str),
    selector_qualifier: Option<&str>,
) -> Result<String> {
    hashed_parts(
        store,
        TARGET_DOMAIN,
        &[
            agent.slug(),
            &path.to_string_lossy(),
            semantic,
            selector_qualifier.unwrap_or_default(),
        ],
    )
}

pub(super) fn physical_selector_qualifier(
    editor: &AgentConfigEditor,
    context: &ConfigContext,
    selector: &str,
) -> Option<String> {
    physical_selector_value(
        selector,
        editor
            .effective(context, ConfigSetting::Model)
            .ok()
            .as_ref()
            .map(|effective| effective.value.as_str()),
    )
    .map(str::to_owned)
}

pub(super) fn physical_selector_value<'a>(
    selector: &str,
    effective_model: Option<&'a str>,
) -> Option<&'a str> {
    matches!(
        selector,
        "modelSettings.effortLevel" | "modelThinkingLevels"
    )
    .then_some(effective_model)
    .flatten()
}

pub(super) fn automatic_editor_supported(
    agent: AgentKind,
    setting: ConfigSetting,
    source: SourceFormat,
    scope: &str,
    environment: &str,
    platform: &str,
) -> bool {
    vendor_policy(agent).is_some_and(|policy| {
        policy.action_support(RemediationAction::AutomaticEdit(setting), source)
            == ActionSupport::Supported
    }) && editor_context_supported(agent, scope, environment, platform)
}

pub(super) fn editor_context_supported(
    _agent: AgentKind,
    scope: &str,
    environment: &str,
    platform: &str,
) -> bool {
    environment == "native"
        && matches!(platform, "macos" | "linux")
        && matches!(scope, "global" | "project")
}

pub(super) fn workspace_precedence_supported(
    agent: AgentKind,
    workspace_candidate: Option<&Path>,
    trusted_root: Option<&Path>,
) -> bool {
    vendor_policy(agent).is_none_or(|policy| {
        policy.workspace_precedence_supported(workspace_candidate, trusted_root)
    })
}

pub(super) fn config_context(
    agent: AgentKind,
    home: &Path,
    workspace_cwd: Option<&Path>,
    trusted_workspace_root: Option<&Path>,
) -> Option<ConfigContext> {
    match (workspace_cwd, trusted_workspace_root) {
        (None, None) => Some(ConfigContext::native(agent, home, None)),
        (Some(cwd), Some(root)) => {
            let cwd = cwd.canonicalize().ok()?;
            let root = root.canonicalize().ok()?;
            cwd.starts_with(&root)
                .then(|| ConfigContext::native_workspace(agent, home, cwd, root))
        }
        _ => None,
    }
}

pub(super) fn workspace_relative_cwd(context: &ConfigContext) -> Option<&str> {
    context
        .workspace_cwd
        .as_deref()?
        .strip_prefix(context.trusted_workspace_root.as_deref()?)
        .ok()?
        .to_str()
}

pub(super) fn recovery_workspace_cwd(root: &Path, relative: Option<&str>) -> Result<PathBuf> {
    let cwd = root.join(relative.unwrap_or_default()).canonicalize()?;
    anyhow::ensure!(
        cwd.starts_with(root),
        "stored workspace cwd escapes its root"
    );
    Ok(cwd)
}

#[cfg(not(windows))]
pub(super) fn apply_prepared_change(
    editor: &AgentConfigEditor,
    prepared: &PreparedChange,
) -> Result<(), crate::agent_config::ApplyError> {
    editor.apply(prepared)
}

#[cfg(windows)]
pub(super) fn apply_prepared_change(
    _: &AgentConfigEditor,
    _: &PreparedChange,
) -> Result<(), crate::agent_config::ApplyError> {
    Err(crate::agent_config::ApplyError::Unavailable(
        crate::agent_config::ConfigUnavailableReason::AutomaticApplyUnsupported,
    ))
}

pub(super) fn refreshed_config_context(context: &ConfigContext) -> ConfigContext {
    let mut current = context.clone();
    current.runtime_override_present = runtime_override_present(current.agent);
    current.managed_configuration_present =
        managed_configuration_present(current.agent, &current.home_root);
    current
}

pub(super) const fn current_editor_platform() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(target_os = "windows")]
    {
        "windows"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "unsupported"
    }
}

pub(super) fn hashed_value(store: &Store, domain: &str, value: &str) -> Result<String> {
    hashed_parts(store, domain.as_bytes(), &[value])
}

pub(super) fn hashed_parts(store: &Store, domain: &[u8], values: &[&str]) -> Result<String> {
    let secret = store.provider_account_secret()?;
    Ok(hashed_parts_with_secret(&secret, domain, values))
}

pub(super) fn hashed_parts_with_secret(
    secret: &[u8; 32],
    domain: &[u8],
    values: &[&str],
) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts a 32-byte key");
    mac.update(domain);
    for value in values {
        mac.update(&(value.len() as u32).to_be_bytes());
        mac.update(value.as_bytes());
    }
    hex(&mac.finalize().into_bytes())
}

pub(super) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

pub(super) fn scope_name(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Global => "global",
        ConfigScope::Project => "project",
    }
}
pub(super) fn scope_from_name(value: &str) -> Option<ConfigScope> {
    match value {
        "global" => Some(ConfigScope::Global),
        "project" => Some(ConfigScope::Project),
        _ => None,
    }
}

pub(crate) fn runtime_override_present(agent: AgentKind) -> bool {
    vendor_policy(agent).is_some_and(|policy| policy.runtime_override_present())
}

pub(crate) fn managed_configuration_present(agent: AgentKind, home: &Path) -> bool {
    vendor_policy(agent).is_some_and(|policy| policy.managed_configuration_present(home))
}
