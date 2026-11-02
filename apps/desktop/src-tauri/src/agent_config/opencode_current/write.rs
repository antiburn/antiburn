use super::super::AgentConfigEditor;
#[cfg(not(windows))]
use super::super::{
    ApplyConflict, ApplyError, ApplyReadbackError, PreparedChange, PreparedOperation,
};
#[cfg(not(windows))]
use super::dependency;
use super::{
    ConfigUnavailableReason, CurrentConfigAuthority, OpenCodeConfigSubject, OpenCodeResolvedTarget,
    OperationSelector, PreparedOpenCodeOperation, inputs, normalize, resolve, skills,
};
use serde_json::Value;

impl AgentConfigEditor {
    pub(crate) fn resolve_opencode(
        &self,
        context: &super::ConfigContext,
        authority: &CurrentConfigAuthority,
        subject: OpenCodeConfigSubject,
    ) -> Result<OpenCodeResolvedTarget, ConfigUnavailableReason> {
        resolve(context, authority, subject)
    }

    #[cfg(windows)]
    pub(crate) fn prepare_opencode_operation(
        &self,
        _: &OpenCodeResolvedTarget,
        _: Value,
    ) -> Result<PreparedOpenCodeOperation, ConfigUnavailableReason> {
        Err(ConfigUnavailableReason::AutomaticApplyUnsupported)
    }

    #[cfg(not(windows))]
    pub(crate) fn prepare_opencode_operation(
        &self,
        target: &OpenCodeResolvedTarget,
        proposed: Value,
    ) -> Result<PreparedOpenCodeOperation, ConfigUnavailableReason> {
        let current = resolve(&target.context, &target.authority, target.subject.clone())?;
        if !target.same_binding(&current)
            || target.dependencies != current.dependencies
            || target.value != current.value
        {
            return Err(ConfigUnavailableReason::ChangedIdentity);
        }
        if !current.reviewed_model_operation_routes(&proposed) {
            return Err(ConfigUnavailableReason::InvalidPrecedence);
        }
        let file = super::read_checked(&target.path, &target.safety_root)?;
        let (proposed_bytes, proposed) = edit(&file.bytes, target, proposed)?;
        if proposed_bytes.len() > 256 * 1024 {
            return Err(ConfigUnavailableReason::FileTooLarge);
        }
        let change = PreparedChange {
            agent: antiburn_local::model::AgentKind::OpenCode,
            setting: target.setting(),
            selector: target.selector.physical_selector(),
            operation: target.selector.clone(),
            expected_value: target.value.to_string(),
            path: target.path.clone(),
            scope: target.scope,
            resolution_home_root: target.context.home_root.clone(),
            workspace_cwd: target.context.workspace_cwd.clone(),
            trusted_workspace_root: target.context.trusted_workspace_root.clone(),
            safety_root: target.safety_root.clone(),
            original_bytes: file.bytes,
            proposed_bytes,
            identity: file.identity,
            permissions: file.permissions,
            #[cfg(unix)]
            ownership: file.ownership,
        };
        let mut current = current;
        current.authority = current.authority.revalidation_only();
        Ok(PreparedOpenCodeOperation {
            target: current,
            additional_targets: Vec::new(),
            operation: PreparedOperation {
                changes: vec![change],
                creations: Vec::new(),
                warning: false,
            },
            proposed,
        })
    }

    #[cfg(not(windows))]
    pub(crate) fn apply_opencode_operation(
        &self,
        prepared: &PreparedOpenCodeOperation,
    ) -> Result<(), ApplyError> {
        let authority = prepared
            .target
            .authority
            .refresh()
            .map_err(|_| ApplyError::Unavailable(ConfigUnavailableReason::RuntimeOverride))?;
        self.revalidate_additional_opencode_targets(prepared, false)
            .map_err(ApplyError::Unavailable)?;
        self.apply_opencode_with_authority(prepared, &authority)?;
        let after = authority
            .refresh()
            .map_err(|_| ApplyError::Readback(ApplyReadbackError::ReadFailed))?;
        self.readback_opencode_with_authority(prepared, &after)?;
        self.revalidate_additional_opencode_targets(prepared, true)
            .map_err(|_| ApplyError::Readback(ApplyReadbackError::SemanticMismatch))
    }

    #[cfg(not(windows))]
    pub(crate) fn revalidate_opencode_operation(
        &self,
        prepared: &PreparedOpenCodeOperation,
    ) -> Result<(), ConfigUnavailableReason> {
        let authority = prepared
            .target
            .authority
            .refresh()
            .map_err(|_| ConfigUnavailableReason::RuntimeOverride)?;
        let current = resolve(
            &prepared.target.context,
            &authority,
            prepared.target.subject.clone(),
        )?;
        if !current.same_binding(&prepared.target)
            || current.value != prepared.target.value
            || current.dependencies != prepared.target.dependencies
        {
            return Err(ConfigUnavailableReason::ChangedIdentity);
        }
        if !current.reviewed_model_operation_routes(&prepared.proposed) {
            return Err(ConfigUnavailableReason::InvalidPrecedence);
        }
        self.revalidate_additional_opencode_targets(prepared, false)?;
        Ok(())
    }

    #[cfg(not(windows))]
    fn revalidate_additional_opencode_targets(
        &self,
        prepared: &PreparedOpenCodeOperation,
        applied: bool,
    ) -> Result<(), ConfigUnavailableReason> {
        for target in &prepared.additional_targets {
            let authority = target
                .authority
                .refresh()
                .map_err(|_| ConfigUnavailableReason::RuntimeOverride)?;
            let current = resolve(&target.context, &authority, target.subject.clone())?;
            if !target.same_binding(&current)
                || current.value
                    != if applied {
                        prepared.proposed.clone()
                    } else {
                        target.value.clone()
                    }
            {
                return Err(ConfigUnavailableReason::ChangedIdentity);
            }
            if !current.reviewed_model_operation_routes(&prepared.proposed) {
                return Err(ConfigUnavailableReason::InvalidPrecedence);
            }
            for expected in &target.dependencies {
                if (!applied || expected.path != target.path)
                    && dependency(&expected.path)? != *expected
                {
                    return Err(ConfigUnavailableReason::ChangedIdentity);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn add_opencode_context(
        &self,
        prepared: &mut PreparedOpenCodeOperation,
        context: &super::ConfigContext,
        authority: &CurrentConfigAuthority,
    ) -> Result<(), ConfigUnavailableReason> {
        let mut target = resolve(context, authority, prepared.target.subject.clone())?;
        if !prepared.target.same_binding(&target) || target.value != prepared.target.value {
            return Err(ConfigUnavailableReason::ChangedIdentity);
        }
        if !target.reviewed_model_operation_routes(&prepared.proposed) {
            return Err(ConfigUnavailableReason::InvalidPrecedence);
        }
        target.authority = target.authority.revalidation_only();
        prepared.additional_targets.push(target);
        Ok(())
    }

    #[cfg(not(windows))]
    fn apply_opencode_with_authority(
        &self,
        prepared: &PreparedOpenCodeOperation,
        authority: &CurrentConfigAuthority,
    ) -> Result<(), ApplyError> {
        let current = resolve(
            &prepared.target.context,
            authority,
            prepared.target.subject.clone(),
        )
        .map_err(ApplyError::Unavailable)?;
        if !current.same_binding(&prepared.target)
            || current.value != prepared.target.value
            || current.dependencies != prepared.target.dependencies
        {
            return Err(ApplyError::Conflict(ApplyConflict::ChangedContent));
        }
        if !current.reviewed_model_operation_routes(&prepared.proposed) {
            return Err(ApplyError::Unavailable(
                ConfigUnavailableReason::InvalidPrecedence,
            ));
        }
        let target = super::super::vendors::Target {
            path: current.path,
            safety_root: current.safety_root,
            scope: current.scope,
            operation: current.selector,
        };
        self.apply_change_at_target_checked(&prepared.operation.changes[0], &target, &|| {
            authority
                .entries()
                .map_err(|_| ApplyError::Unavailable(ConfigUnavailableReason::RuntimeOverride))?;
            for expected in &prepared.target.dependencies {
                if dependency(&expected.path).map_err(ApplyError::Unavailable)? != *expected {
                    return Err(ApplyError::Conflict(ApplyConflict::ChangedContent));
                }
            }
            for target in &prepared.additional_targets {
                for expected in &target.dependencies {
                    if dependency(&expected.path).map_err(ApplyError::Unavailable)? != *expected {
                        return Err(ApplyError::Conflict(ApplyConflict::ChangedContent));
                    }
                }
            }
            Ok(())
        })
    }

    #[cfg(all(test, not(windows)))]
    pub(crate) fn readback_opencode_operation(
        &self,
        prepared: &PreparedOpenCodeOperation,
    ) -> Result<(), ApplyError> {
        let authority = prepared
            .target
            .authority
            .refresh()
            .map_err(|_| ApplyError::Readback(ApplyReadbackError::ReadFailed))?;
        self.readback_opencode_with_authority(prepared, &authority)
    }

    #[cfg(not(windows))]
    fn readback_opencode_with_authority(
        &self,
        prepared: &PreparedOpenCodeOperation,
        authority: &CurrentConfigAuthority,
    ) -> Result<(), ApplyError> {
        let current = resolve(
            &prepared.target.context,
            authority,
            prepared.target.subject.clone(),
        )
        .map_err(|_| ApplyError::Readback(ApplyReadbackError::ReadFailed))?;
        let change = &prepared.operation.changes[0];
        if !prepared.target.same_binding(&current)
            || current.value != prepared.proposed
            || !current.reviewed_model_operation_routes(&prepared.proposed)
        {
            return Err(ApplyError::Readback(ApplyReadbackError::SemanticMismatch));
        }
        for expected in &prepared.target.dependencies {
            if expected.path != prepared.target.path
                && dependency(&expected.path)
                    .map_err(|_| ApplyError::Readback(ApplyReadbackError::ReadFailed))?
                    != *expected
            {
                return Err(ApplyError::Readback(ApplyReadbackError::ChangedContent));
            }
        }
        let actual = super::read_checked(&change.path, &change.safety_root)
            .map_err(|_| ApplyError::Readback(ApplyReadbackError::ReadFailed))?;
        if actual.bytes != change.proposed_bytes {
            return Err(ApplyError::Readback(ApplyReadbackError::ChangedContent));
        }
        Ok(())
    }
}

#[cfg(windows)]
impl AgentConfigEditor {
    pub(crate) fn revalidate_opencode_operation(
        &self,
        _: &PreparedOpenCodeOperation,
    ) -> Result<(), ConfigUnavailableReason> {
        Err(ConfigUnavailableReason::AutomaticApplyUnsupported)
    }
}

pub(in crate::agent_config) fn read_value(
    bytes: &[u8],
    selector: &OperationSelector,
) -> Result<Option<Value>, ConfigUnavailableReason> {
    let subject = match selector {
        OperationSelector::OpenCodeDefaultModel => OpenCodeConfigSubject::FutureDefaultModel,
        OperationSelector::OpenCodeAgentModel { agent, .. } => {
            OpenCodeConfigSubject::AgentModel(agent.clone())
        }
        OperationSelector::OpenCodeAutomaticCompaction => {
            OpenCodeConfigSubject::AutomaticCompaction
        }
        OperationSelector::OpenCodeMcpServer { server, .. } => {
            OpenCodeConfigSubject::McpServer(server.clone())
        }
        OperationSelector::OpenCodeSkillAdvertising(_) => return skills::value(bytes).map(Some),
        OperationSelector::OpenCodeMarkdownAgentModel(_) => {
            let (header, _) = inputs::markdown(bytes)?;
            return normalize::markdown_agent_model(&header)?
                .ok_or(ConfigUnavailableReason::MissingTarget)
                .map(Some);
        }
        _ => return Ok(None),
    };
    normalize::select(&super::super::vendors::json::parse(bytes)?, &subject)
        .map(|selected| selected.map(|(_, value)| value))
}

#[cfg(not(windows))]
fn edit(
    bytes: &[u8],
    target: &OpenCodeResolvedTarget,
    proposed: Value,
) -> Result<(Vec<u8>, Value), ConfigUnavailableReason> {
    let mut document = if target
        .path
        .extension()
        .is_some_and(|extension| extension == "md")
    {
        Value::Null
    } else {
        super::super::vendors::json::parse(bytes)?
    };
    let mut canonical = proposed.clone();
    let output = match &target.subject {
        OpenCodeConfigSubject::FutureDefaultModel | OpenCodeConfigSubject::AgentModel(_) => {
            canonical = normalize::model(&proposed)?;
            if canonical == target.value
                || canonical.get("providerID") != target.value.get("providerID")
                || canonical.get("variant") != target.value.get("variant")
            {
                return Err(ConfigUnavailableReason::InvalidTarget);
            }
            if matches!(
                target.selector,
                OperationSelector::OpenCodeMarkdownAgentModel(_)
            ) {
                inputs::markdown_edit(bytes, |header| {
                    let raw = serde_json::to_value(
                        header
                            .get("model")
                            .ok_or(ConfigUnavailableReason::MissingTarget)?,
                    )
                    .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
                    header["model"] = serde_yaml_ng::to_value(encode_model(&raw, &canonical)?)
                        .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
                    Ok(())
                })?
            } else {
                let mut path = target.selector.exact_path();
                let value = value_mut(&mut document, &path)?;
                let encoded = if value.is_object() {
                    path.push("model".into());
                    canonical["model"].clone()
                } else {
                    encode_model(value, &canonical)?
                };
                super::super::vendors::json::edit_path_value(bytes, &path, &encoded, false)?
            }
        }
        OpenCodeConfigSubject::AutomaticCompaction => {
            if target.value != Value::Bool(false) || proposed != Value::Bool(true) {
                return Err(ConfigUnavailableReason::InvalidTarget);
            }
            super::super::vendors::json::edit_path_value(
                bytes,
                &target.selector.exact_path(),
                &proposed,
                false,
            )?
        }
        OpenCodeConfigSubject::McpServer(_) => {
            if proposed != Value::Bool(false)
                || target.value.get("disabled").and_then(Value::as_bool) == Some(true)
            {
                return Err(ConfigUnavailableReason::InvalidTarget);
            }
            let path = target.selector.exact_path();
            let field = path.last().ok_or(ConfigUnavailableReason::InvalidTarget)?;
            canonical = target.value.clone();
            canonical["disabled"] = Value::Bool(true);
            super::super::vendors::json::edit_path_value(
                bytes,
                &path,
                &Value::Bool(field == "disabled"),
                true,
            )?
        }
        OpenCodeConfigSubject::SkillAdvertising(_) => {
            if target.value != Value::Bool(true) || proposed != Value::Bool(false) {
                return Err(ConfigUnavailableReason::InvalidTarget);
            }
            skills::edit(bytes)?
        }
    };
    Ok((output, canonical))
}

#[cfg(not(windows))]
fn encode_model(raw: &Value, proposed: &Value) -> Result<Value, ConfigUnavailableReason> {
    if raw.is_object() {
        let mut encoded = raw.clone();
        encoded["providerID"] = proposed["providerID"].clone();
        encoded["model"] = proposed["model"].clone();
        return Ok(encoded);
    }
    let provider = proposed["providerID"]
        .as_str()
        .ok_or(ConfigUnavailableReason::InvalidTarget)?;
    let model = proposed["model"]
        .as_str()
        .ok_or(ConfigUnavailableReason::InvalidTarget)?;
    let variant = proposed
        .get("variant")
        .and_then(Value::as_str)
        .filter(|_| raw.as_str().is_some_and(|raw| raw.contains('#')))
        .map(|variant| format!("#{variant}"))
        .unwrap_or_default();
    Ok(format!("{provider}/{model}{variant}").into())
}

#[cfg(not(windows))]
fn value_mut<'a>(
    mut value: &'a mut Value,
    path: &[String],
) -> Result<&'a mut Value, ConfigUnavailableReason> {
    for key in path {
        value = value
            .get_mut(key)
            .ok_or(ConfigUnavailableReason::MissingTarget)?;
    }
    Ok(value)
}

#[cfg(all(test, not(windows)))]
pub(super) fn apply_fixture(
    editor: &AgentConfigEditor,
    prepared: &PreparedOpenCodeOperation,
    authority: &CurrentConfigAuthority,
) -> Result<(), ApplyError> {
    editor.apply_opencode_with_authority(prepared, authority)
}
#[cfg(all(test, not(windows)))]
pub(super) fn readback_fixture(
    editor: &AgentConfigEditor,
    prepared: &PreparedOpenCodeOperation,
    authority: &CurrentConfigAuthority,
) -> Result<(), ApplyError> {
    editor.readback_opencode_with_authority(prepared, authority)
}
