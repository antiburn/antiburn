use super::*;

pub(super) fn opencode_subject(cause: &FindingCause) -> Option<OpenCodeConfigSubject> {
    match cause {
        FindingCause::OldModelUsage { .. } => Some(OpenCodeConfigSubject::FutureDefaultModel),
        FindingCause::SessionsOverDepth { .. } => Some(OpenCodeConfigSubject::AutomaticCompaction),
        FindingCause::OverpoweredSubagents {
            worker_model,
            config_binding: Some(binding),
            ..
        } if worker_model == &binding.model_id => {
            Some(OpenCodeConfigSubject::AgentModel(binding.agent_id.clone()))
        }
        FindingCause::UnusedMcpServer { server, .. } => {
            Some(OpenCodeConfigSubject::McpServer(server.clone()))
        }
        FindingCause::UnusedSkill { skill, .. } => {
            Some(OpenCodeConfigSubject::SkillAdvertising(skill.clone()))
        }
        _ => None,
    }
}

pub(super) fn opencode_recovery_subject(
    definition: &WatchDefinition,
    setting: ConfigSetting,
) -> Option<OpenCodeConfigSubject> {
    match setting {
        ConfigSetting::Model => Some(OpenCodeConfigSubject::FutureDefaultModel),
        ConfigSetting::SubagentModel => {
            let identity: serde_json::Value =
                serde_json::from_str(&definition.canonical_identity).ok()?;
            let binding: antiburn_local::analysis::SubagentConfigBinding =
                serde_json::from_value(identity.get("configuredAgent")?.clone()).ok()?;
            let expected = format!("{}/{}", binding.provider_id, binding.model_id);
            (definition.config_expected_value.as_deref() == Some(expected.as_str()))
                .then_some(OpenCodeConfigSubject::AgentModel(binding.agent_id))
        }
        ConfigSetting::Compaction => Some(OpenCodeConfigSubject::AutomaticCompaction),
        ConfigSetting::McpServer => definition
            .resource
            .clone()
            .map(OpenCodeConfigSubject::McpServer),
        ConfigSetting::Skill => definition
            .resource
            .clone()
            .map(OpenCodeConfigSubject::SkillAdvertising),
        _ => None,
    }
}

pub(super) fn opencode_proposed_value(operation: &ConfigOperation) -> Option<serde_json::Value> {
    match (&operation.setting, &operation.proposed_value) {
        (ConfigSetting::Model, crate::agent_config::ConfigOperationValue::Scalar(value)) => {
            Some(serde_json::Value::String(value.clone()))
        }
        (ConfigSetting::Compaction, crate::agent_config::ConfigOperationValue::Boolean(value)) => {
            Some(serde_json::Value::Bool(*value))
        }
        (
            ConfigSetting::SubagentModel,
            crate::agent_config::ConfigOperationValue::Scalar(value),
        ) => Some(serde_json::Value::String(value.clone())),
        (
            ConfigSetting::McpServer | ConfigSetting::Skill,
            crate::agent_config::ConfigOperationValue::MapEntry { value, .. },
        ) if value == "false" => Some(serde_json::Value::Bool(false)),
        _ => None,
    }
}

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
    pub records: Vec<PublicationSettingAttribution>,
}

#[derive(Clone)]
pub(crate) struct PublicationSettingAttribution {
    pub setting: ConfigSetting,
    pub path: String,
    pub selector: String,
    pub expected_value_json: String,
    pub precedence_hash: String,
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
    publication_config_attribution_with_authority(store, key, status, evidence_json, &home, None)
}

/// Capture live authority in the worker, before the storage transaction.
pub(crate) async fn publication_config_attribution_async(
    cache: &mut super::PublicationAuthorityCache,
    store: &Store,
    key: &crate::store::SessionKey,
    status: crate::store::PublishedEvidence,
    evidence_json: &str,
) -> Result<PublicationConfigAttribution> {
    let store = store.clone();
    let key = key.clone();
    let evidence_json = evidence_json.to_owned();
    let prepared = publication_config_io(move || {
        let Some(home) = antiburn_local::paths::home_dir() else {
            return Ok(None);
        };
        let location = publication_authority_location(&store, &key, status, &evidence_json, &home)?;
        Ok(Some((store, key, evidence_json, home, location)))
    })
    .await?;
    let Some((store, key, evidence_json, home, location)) = prepared else {
        return Ok(PublicationConfigAttribution::default());
    };
    let authority = match location {
        Some(cwd) => cache.get_async(&cwd).await.cloned(),
        None => None,
    };
    publication_config_io(move || {
        publication_config_attribution_with_authority(
            &store,
            &key,
            status,
            &evidence_json,
            &home,
            authority.as_ref(),
        )
    })
    .await
}

async fn publication_config_io<T: Send + 'static>(
    read: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(read)
        .await
        .context("publication config task failed")?
}

fn publication_authority_location(
    store: &Store,
    key: &crate::store::SessionKey,
    status: crate::store::PublishedEvidence,
    evidence_json: &str,
    home: &Path,
) -> Result<Option<PathBuf>> {
    if key.agent != "opencode"
        || key.environment_key != "native"
        || status != crate::store::PublishedEvidence::Ready
    {
        return Ok(None);
    }
    let evidence: antiburn_local::analysis::SessionEvidence = serde_json::from_str(evidence_json)?;
    if !matches!(&evidence.models, antiburn_local::analysis::EvidenceValue::Complete(models) if !models.control_observations.is_empty())
    {
        return Ok(None);
    }
    let Some(policy) = vendor_policy(AgentKind::OpenCode) else {
        return Ok(None);
    };
    if policy.action_support(
        RemediationAction::PublicationAttribution(ConfigSetting::Model),
        evidence.capabilities.source_format,
    ) == ActionSupport::Unsupported
    {
        return Ok(None);
    }
    let Some(cwd) = store
        .session(key)?
        .and_then(|session| session.cwd)
        .map(PathBuf::from)
    else {
        return Ok(None);
    };
    let Some(root) = trusted_workspace(store, &cwd).ok().flatten() else {
        return Ok(None);
    };
    if !workspace_precedence_supported(AgentKind::OpenCode, Some(&cwd), Some(&root))
        || config_context(AgentKind::OpenCode, home, Some(&cwd), Some(&root)).is_none()
        || runtime_override_present(AgentKind::OpenCode)
        || managed_configuration_present(AgentKind::OpenCode, home)
    {
        return Ok(None);
    }
    Ok(Some(cwd))
}

#[cfg(test)]
pub(crate) fn publication_config_attribution_with_home(
    store: &Store,
    key: &crate::store::SessionKey,
    status: crate::store::PublishedEvidence,
    evidence_json: &str,
    home: &Path,
) -> Result<PublicationConfigAttribution> {
    publication_config_attribution_with_authority(store, key, status, evidence_json, home, None)
}

pub(crate) fn publication_config_attribution_with_authority(
    store: &Store,
    key: &crate::store::SessionKey,
    status: crate::store::PublishedEvidence,
    evidence_json: &str,
    home: &Path,
    authority: Option<&crate::agent_config::opencode_config_authority::CurrentConfigAuthority>,
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
    let evidence: antiburn_local::analysis::SessionEvidence = serde_json::from_str(evidence_json)?;
    let source = evidence.capabilities.source_format;
    if [ConfigSetting::Model, ConfigSetting::Reasoning]
        .into_iter()
        .all(|setting| {
            policy.action_support(RemediationAction::PublicationAttribution(setting), source)
                == ActionSupport::Unsupported
        })
    {
        return Ok(PublicationConfigAttribution::default());
    }
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
    context.source_format = Some(source);
    context.managed_configuration_present =
        managed_configuration_present(agent, &context.home_root);
    let editor = AgentConfigEditor::new();
    if agent == AgentKind::OpenCode {
        return opencode_publication_attribution(store, &context, &evidence, authority);
    }
    let effective_model = editor.effective(&context, ConfigSetting::Model).ok();
    let effective_reasoning = editor.effective(&context, ConfigSetting::Reasoning).ok();
    if effective_model.is_none() && effective_reasoning.is_none() {
        return Ok(PublicationConfigAttribution::default());
    }
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
    let mut records = Vec::new();
    if let Some(record) = model.as_ref().map(|value| value.record.clone()) {
        records.push(record);
    }
    if let Some(record) = reasoning.as_ref().map(|value| value.record.clone()) {
        records.push(record);
    }
    Ok(PublicationConfigAttribution {
        model: model.map(|value| value.legacy),
        reasoning: reasoning.map(|value| value.legacy),
        records,
    })
}

fn opencode_publication_attribution(
    store: &Store,
    context: &ConfigContext,
    evidence: &antiburn_local::analysis::SessionEvidence,
    authority: Option<&crate::agent_config::opencode_config_authority::CurrentConfigAuthority>,
) -> Result<PublicationConfigAttribution> {
    let Some(authority) = authority else {
        return Ok(PublicationConfigAttribution::default());
    };
    let Ok(resolved) = AgentConfigEditor::new().resolve_opencode(
        context,
        authority,
        OpenCodeConfigSubject::FutureDefaultModel,
    ) else {
        return Ok(PublicationConfigAttribution::default());
    };
    let Some(value) = resolved.current_value_display() else {
        return Ok(PublicationConfigAttribution::default());
    };
    let antiburn_local::analysis::EvidenceValue::Complete(models) = &evidence.models else {
        return Ok(PublicationConfigAttribution::default());
    };
    let policy = vendor_policy(AgentKind::OpenCode).expect("OpenCode has a remediation policy");
    if !resolved.reviewed_default_route()
        || !policy.publication_setting_observed(
            ConfigSetting::Model,
            &value,
            Some(&value),
            &models.control_observations,
        )
    {
        return Ok(PublicationConfigAttribution::default());
    }
    let (path, selector) = resolved.physical_identity();
    let target_hash = physical_key(store, AgentKind::OpenCode, (path, selector), None)?;
    let scope = scope_name(resolved.scope()).to_owned();
    Ok(PublicationConfigAttribution {
        model: Some((target_hash, scope, value.clone())),
        reasoning: None,
        records: vec![PublicationSettingAttribution {
            setting: ConfigSetting::Model,
            path: path.to_string_lossy().into_owned(),
            selector: selector.to_owned(),
            expected_value_json: serde_json::to_string(&value)?,
            precedence_hash: hex(&resolved.resolution_digest()),
        }],
    })
}

struct AttributedSetting {
    legacy: (String, String, String),
    record: PublicationSettingAttribution,
}

fn publication_setting_attribution(
    store: &Store,
    policy: &dyn vendors::VendorRemediationPolicy,
    agent: AgentKind,
    source: SourceFormat,
    effective: Option<&crate::agent_config::EffectiveConfig>,
    effective_model: Option<&str>,
    observations: &[antiburn_local::analysis::ModelControlObservation],
) -> Result<Option<AttributedSetting>> {
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
    let selector_qualifier =
        physical_selector_value(effective.physical_identity().1, effective_model);
    let target_hash = physical_key(
        store,
        agent,
        effective.physical_identity(),
        selector_qualifier,
    )?;
    let scope = scope_name(effective.scope).to_owned();
    let expected_value_json = serde_json::to_string(&effective.value)?;
    let (path, selector) = effective.physical_identity();
    let precedence_hash = hashed_parts(
        store,
        b"config-precedence",
        &[
            agent.slug(),
            &path.to_string_lossy(),
            selector,
            selector_qualifier.unwrap_or_default(),
            &expected_value_json,
        ],
    )?;
    Ok(Some(AttributedSetting {
        legacy: (target_hash, scope.clone(), effective.value.clone()),
        record: PublicationSettingAttribution {
            setting: effective.setting,
            path: path.to_string_lossy().into_owned(),
            selector: selector.to_owned(),
            expected_value_json,
            precedence_hash,
        },
    }))
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

pub(crate) fn config_context(
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

pub(super) fn workspace_relative_cwd(context: &ConfigContext) -> Option<String> {
    context
        .workspace_cwd
        .as_deref()?
        .strip_prefix(context.trusted_workspace_root.as_deref()?)
        .ok()?
        .to_str()
        .map(|path| path.replace('\\', "/"))
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
    prepared: &super::PreparedConfigOperation,
) -> Result<(), crate::agent_config::ApplyError> {
    match prepared {
        super::PreparedConfigOperation::Generic(prepared) => editor.apply(prepared),
        super::PreparedConfigOperation::OpenCode(prepared) => {
            editor.apply_opencode_operation(prepared)
        }
    }
}

#[cfg(windows)]
pub(super) fn apply_prepared_change(
    _: &AgentConfigEditor,
    _: &super::PreparedConfigOperation,
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

#[cfg(test)]
mod publication_precondition_tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn publication_config_io_keeps_the_runtime_responsive_during_blocked_work() {
        use std::time::Instant;

        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("config");
        std::fs::write(&path, b"x").unwrap();
        let (release, wait) = std::sync::mpsc::channel();
        let (entered, reading) = tokio::sync::oneshot::channel();
        let caller = std::thread::current().id();
        let read = tokio::spawn(publication_config_io(move || {
            assert_ne!(std::thread::current().id(), caller);
            entered.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(5))?;
            Ok(std::fs::read(path)?)
        }));
        reading.await.unwrap();
        let started = Instant::now();
        tokio::time::sleep(Duration::from_millis(10)).await;
        let responsive = started.elapsed() < Duration::from_secs(1);
        let _ = release.send(());
        assert_eq!(read.await.unwrap().unwrap(), b"x");
        assert!(
            responsive,
            "config file reads must not block the runtime thread"
        );
    }

    #[test]
    fn publication_without_model_controls_or_session_location_does_not_need_authority() {
        let temporary = tempfile::tempdir().unwrap();
        let store = Store::open(temporary.path()).unwrap();
        let key = crate::store::SessionKey::new("native", "opencode", "missing");
        let mut evidence =
            super::super::opencode_v2_tests::model_evidence(SourceFormat::OpenCodeSqliteV2);
        let json = serde_json::to_string(&evidence).unwrap();
        assert!(
            publication_authority_location(
                &store,
                &key,
                crate::store::PublishedEvidence::Ready,
                &json,
                temporary.path()
            )
            .unwrap()
            .is_none()
        );
        if let antiburn_local::analysis::EvidenceValue::Complete(models) = &mut evidence.models {
            models.control_observations.clear();
        }
        assert!(
            publication_authority_location(
                &store,
                &key,
                crate::store::PublishedEvidence::Ready,
                &serde_json::to_string(&evidence).unwrap(),
                temporary.path()
            )
            .unwrap()
            .is_none()
        );
        let key = crate::store::SessionKey::new("wsl:Ubuntu", "opencode", "missing");
        assert!(
            publication_authority_location(
                &store,
                &key,
                crate::store::PublishedEvidence::Ready,
                "not JSON",
                temporary.path()
            )
            .unwrap()
            .is_none()
        );
    }
}
