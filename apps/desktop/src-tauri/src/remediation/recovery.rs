use super::*;

pub(crate) fn recover_uncertain_write(
    store: &Store,
    record: &RemediationRecord,
    now: i64,
) -> Result<bool> {
    validate_envelope_version(&record.result_json, "remediation result")?;
    if record.environment_key != "native" {
        return store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        );
    }
    let definition = parse_watch_definition(&record.definition_json)?;
    let Some(replacement) = definition
        .config_proposed_value
        .clone()
        .or(definition.replacement.clone())
    else {
        return store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        );
    };
    let Some(agent) = crate::agents::kind_from_slug(&record.agent) else {
        return store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "unsupportedAgent",
            now,
        );
    };
    let Some(policy) = vendor_policy(agent) else {
        return store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        );
    };
    let setting = definition
        .config_setting
        .as_deref()
        .and_then(config_setting_from_name)
        .or_else(|| {
            definition
                .old_model
                .is_some()
                .then_some(ConfigSetting::Model)
        });
    let Some(setting) = setting else {
        return store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        );
    };
    if policy.action_support(
        RemediationAction::RecoverUncertainWrite(setting),
        definition.source_format.value(),
    ) == ActionSupport::Unsupported
    {
        return store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        );
    }
    if !editor_context_supported(
        agent,
        &record.scope_kind,
        &record.environment_key,
        current_editor_platform(),
    ) {
        return store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        );
    }
    let Some(home) = antiburn_local::paths::home_dir() else {
        return store.defer_remediation_recovery(&record.remediation_id, "homeUnavailable", now);
    };
    let trusted_root = if definition.workspace_key.is_some() {
        store
            .repositories()?
            .into_iter()
            .filter_map(|repository| repository.repo_root)
            .filter_map(|root| PathBuf::from(root).canonicalize().ok())
            .find(|project| {
                definition.workspace_key.as_deref()
                    == hashed_workspace_key(store, project).ok().as_deref()
            })
    } else {
        None
    };
    if definition.workspace_key.is_some() && trusted_root.is_none() {
        return store.defer_remediation_recovery(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        );
    }
    let workspace_cwd = trusted_root
        .as_ref()
        .map(|root| recovery_workspace_cwd(root, definition.workspace_relative_cwd.as_deref()))
        .transpose()?;
    let Some(mut context) = config_context(
        agent,
        &home,
        workspace_cwd.as_deref(),
        trusted_root.as_deref(),
    ) else {
        return store.defer_remediation_recovery(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        );
    };
    context.runtime_override_present = runtime_override_present(agent);
    context.managed_configuration_present = managed_configuration_present(agent, &home);
    let Ok(effective) = AgentConfigEditor::new().effective(&context, setting) else {
        return store.defer_remediation_recovery(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        );
    };
    let selector_qualifier = physical_selector_qualifier(
        &AgentConfigEditor::new(),
        &context,
        effective.physical_identity().1,
    );
    let key = physical_key(
        store,
        agent,
        effective.physical_identity(),
        selector_qualifier.as_deref(),
    )?;
    if !recovery_target_matches(
        definition.physical_target_key.as_deref(),
        &record.scope_kind,
        &key,
        effective.scope,
    ) {
        return store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "physicalTargetChanged",
            now,
        );
    }
    match effective.value {
        value if value == replacement => {
            store.finalize_remediation_write(&record.remediation_id, now.saturating_mul(1_000), now)
        }
        _ => store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "writeOutcomeUnknown",
            now,
        ),
    }
}

pub(super) fn recovery_target_matches(
    stored_target: Option<&str>,
    stored_scope: &str,
    effective_target: &str,
    effective_scope: ConfigScope,
) -> bool {
    stored_target == Some(effective_target) && stored_scope == scope_name(effective_scope)
}
