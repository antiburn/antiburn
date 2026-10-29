use super::*;

#[cfg(test)]
pub(crate) fn recover_uncertain_write(
    store: &Store,
    record: &RemediationRecord,
    now: i64,
) -> Result<bool> {
    recover_uncertain_write_with_cache(
        store,
        record,
        now,
        &mut opencode_current::AuthorityCache::new(),
    )
}

pub(crate) fn recover_uncertain_write_with_cache(
    store: &Store,
    record: &RemediationRecord,
    now: i64,
    authorities: &mut opencode_current::AuthorityCache,
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
    if agent == AgentKind::OpenCode
        && (definition.workspace_key.is_none()
            || opencode_recovery_subject(&definition, setting).is_none())
    {
        return store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        );
    }
    if matches!(
        setting,
        ConfigSetting::McpServer | ConfigSetting::BuiltInTool | ConfigSetting::Skill
    ) && resource_replacement_value(&replacement, definition.resource.as_deref()).is_none()
    {
        return store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        );
    }
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
    context.source_format = Some(definition.source_format.value());
    context.managed_configuration_present = managed_configuration_present(agent, &home);
    let (key, scope, current) = if agent == AgentKind::OpenCode {
        let subject = opencode_recovery_subject(&definition, setting);
        let Some((cwd, subject)) = context.workspace_cwd.as_deref().zip(subject) else {
            return recovery_authority_unavailable(
                store,
                record,
                crate::agent_config::opencode_config_authority::CaptureUnavailable::Location,
                now,
            );
        };
        let authority = match authorities.get_for_recovery(cwd) {
            Ok(authority) => authority,
            Err(reason) => return recovery_authority_unavailable(store, record, reason, now),
        };
        let resolved = AgentConfigEditor::new()
            .resolve_opencode(&context, authority, subject)
            .ok();
        let Some(resolved) = resolved else {
            return store.mark_remediation_recovery_checked(
                &record.remediation_id,
                "verificationUnavailable",
                now,
            );
        };
        let key = physical_key(
            store,
            agent,
            resolved.physical_identity(),
            resolved.selector_qualifier(),
        )?;
        let current = resolved.current_value_display().or_else(|| {
            resolved
                .current_value()
                .as_bool()
                .zip(definition.resource.as_deref())
                .map(|(value, resource)| format!("{resource}={value}"))
        });
        (key, resolved.scope(), current)
    } else {
        let expected_target = match setting {
            ConfigSetting::McpServer | ConfigSetting::BuiltInTool | ConfigSetting::Skill => {
                definition.resource.as_deref()
            }
            _ => definition.config_expected_value.as_deref(),
        };
        let Ok(effective) =
            AgentConfigEditor::new().effective_for_value(&context, setting, expected_target)
        else {
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
        (key, effective.scope, Some(effective.value))
    };
    if !recovery_target_matches(
        definition.physical_target_key.as_deref(),
        &record.scope_kind,
        &key,
        scope,
    ) {
        return store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "physicalTargetChanged",
            now,
        );
    }
    match current {
        Some(value) if value == replacement => {
            let verification_available =
                DetectorId::from_key(&definition.detector).is_some_and(|detector| {
                    super::watch_verification_available(
                        &definition,
                        &record.scope_kind,
                        &record.agent,
                        detector,
                    )
                });
            store.finalize_remediation_write(
                &record.remediation_id,
                now.saturating_mul(1_000),
                now,
                verification_available,
            )
        }
        _ => store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "writeOutcomeUnknown",
            now,
        ),
    }
}

fn recovery_authority_unavailable(
    store: &Store,
    record: &RemediationRecord,
    reason: crate::agent_config::opencode_config_authority::CaptureUnavailable,
    now: i64,
) -> Result<bool> {
    if reason == crate::agent_config::opencode_config_authority::CaptureUnavailable::Budget {
        store.defer_remediation_recovery(&record.remediation_id, "verificationUnavailable", now)
    } else {
        store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        )
    }
}

pub(super) fn resource_replacement_value<'a>(
    replacement: &'a str,
    expected_resource: Option<&str>,
) -> Option<&'a str> {
    let (resource, value) = replacement.split_once('=')?;
    (Some(resource) == expected_resource).then_some(value)
}

pub(super) fn recovery_target_matches(
    stored_target: Option<&str>,
    stored_scope: &str,
    effective_target: &str,
    effective_scope: ConfigScope,
) -> bool {
    stored_target == Some(effective_target) && stored_scope == scope_name(effective_scope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_opencode_writes_receive_one_terminal_check_without_writing_files() {
        for (setting, expected, workspace) in [
            ("model", "openai/gpt-5.5", None),
            ("subagentModel", "gpt-5.5", Some("legacy-workspace")),
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let config = temporary.path().join("opencode.json");
            std::fs::write(&config, r#"{"model":"openai/gpt-5.5"}"#).unwrap();
            let store = Store::open(temporary.path()).unwrap();
            let definition = json!({
                "version": 1, "detector": "old_model_usage", "canonicalIdentity": "{}",
                "sourceFormat": SourceFormat::OpenCodeSqliteV1, "workspaceKey": workspace,
                "physicalTargetKey": "legacy-target", "configSetting": setting,
                "configExpectedValue": expected, "configProposedValue": "openai/gpt-5.6-luna",
                "verificationMethodRevision": VERIFICATION_METHOD_REVISION,
                "remediationPolicyRevision": REMEDIATION_POLICY_REVISION,
                "savingsMethodRevision": SAVINGS_METHOD_REVISION
            });
            // Insert an old record without passing the current enrollment gate.
            store.lock().execute(
                "INSERT INTO remediation (remediation_id, target_key, environment_key, agent,
                 scope_kind, scope_key, state, definition_json, result_json,
                 created_at_epoch, updated_at_epoch, origin)
                 VALUES ('legacy', 'target', 'native', 'opencode', 'global', 'legacy-target',
                 'reserved', ?1, ?2, 1, 1, 'action')",
                rusqlite::params![definition.to_string(), json!({"version":1,"verification":{"status":"watching"},"savings":{"status":"unavailable"}}).to_string()],
            ).unwrap();
            let record = store.remediation("legacy").unwrap().unwrap();
            assert!(
                store
                    .begin_remediation_write(&record.remediation_id, 2)
                    .unwrap()
            );
            assert!(
                store
                    .mark_remediation_recovery_needed(&record.remediation_id, "readbackFailed", 2)
                    .unwrap()
            );
            let record = store.remediation(&record.remediation_id).unwrap().unwrap();
            assert!(
                recovery_authority_unavailable(
                    &store,
                    &record,
                    crate::agent_config::opencode_config_authority::CaptureUnavailable::Budget,
                    2,
                )
                .unwrap()
            );
            let deferred = store.remediation(&record.remediation_id).unwrap().unwrap();
            let result: serde_json::Value = serde_json::from_str(&deferred.result_json).unwrap();
            assert_eq!(result["verification"]["retryAfterEpoch"], 62);
            assert!(result["verification"].get("checkedAtEpoch").is_none());
            assert!(recover_uncertain_write(&store, &record, 3).unwrap());
            let after = store.remediation(&record.remediation_id).unwrap().unwrap();
            let result: serde_json::Value = serde_json::from_str(&after.result_json).unwrap();
            assert_eq!(result["verification"]["checkedAtEpoch"], 3);
            assert_eq!(result["verification"]["reason"], "verificationUnavailable");
            assert!(result["verification"].get("retryAfterEpoch").is_none());
            assert!(after.effective_boundary_ms.is_none());
            assert_eq!(
                std::fs::read_to_string(&config).unwrap(),
                r#"{"model":"openai/gpt-5.5"}"#
            );
        }
    }
}
