use super::*;

pub(super) fn watch_definition(target: &CachedTarget) -> WatchDefinition {
    let (provider, api, old_model, replacement) = match target.findings[0].finding.cause() {
        FindingCause::OldModelUsage {
            provider,
            api,
            model,
            replacement,
            ..
        } => (
            provider.clone(),
            api.clone(),
            Some(model.clone()),
            Some(replacement.clone()),
        ),
        FindingCause::ModelOverthinking { provider, api, .. }
        | FindingCause::OveruseOfFastMode { provider, api, .. } => {
            (provider.clone(), api.clone(), None, None)
        }
        _ => (None, None, None, None),
    };
    let (target_model, target_control) = match target.findings[0].finding.cause() {
        FindingCause::ModelOverthinking {
            model, reasoning, ..
        } => (Some(model.clone()), Some(reasoning.clone())),
        FindingCause::OveruseOfFastMode { model, .. } => {
            (Some(model.clone()), Some("fast".to_owned()))
        }
        _ => (None, None),
    };
    let old_pricing = old_model.as_deref().and_then(|model| {
        reviewed_pricing(target.agent, provider.as_deref(), api.as_deref(), model)
    });
    let replacement_pricing = replacement.as_deref().and_then(|model| {
        reviewed_pricing(target.agent, provider.as_deref(), api.as_deref(), model)
    });
    WatchDefinition {
        version: 1,
        detector: target.findings[0].finding.detector.key().into(),
        canonical_identity: target.canonical_identity.clone(),
        source_format: target.findings[0].finding.source_format.into(),
        workspace_key: target.workspace_key.clone(),
        workspace_relative_cwd: target
            .config
            .as_ref()
            .and_then(|config| workspace_relative_cwd(&config.context)),
        provider,
        api,
        old_model,
        replacement,
        resource: match target.findings[0].finding.cause() {
            FindingCause::UnusedMcpServer { server } => Some(server.clone()),
            FindingCause::UnusedBuiltInTool { tool, .. } => Some(tool.clone()),
            FindingCause::UnusedSkill { skill } => Some(skill.clone()),
            _ => None,
        },
        physical_target_key: target.physical_target_key.clone(),
        config_setting: target.physical_target_key.as_ref().and_then(|_| {
            match target.findings[0].finding.cause() {
                FindingCause::OldModelUsage { .. } => Some("model".to_owned()),
                FindingCause::ModelOverthinking { .. } => Some("reasoning".to_owned()),
                _ => None,
            }
        }),
        config_expected_value: target
            .config
            .as_ref()
            .map(|config| config.operation.expected_value.clone()),
        config_proposed_value: target
            .config
            .as_ref()
            .map(|config| config.operation.proposed_value.clone()),
        verification_method_revision: VERIFICATION_METHOD_REVISION,
        remediation_policy_revision: Some(REMEDIATION_POLICY_REVISION),
        savings_method_revision: SAVINGS_METHOD_REVISION,
        pricing_revision: (old_pricing.is_some() && replacement_pricing.is_some()).then(|| {
            format!(
                "pricing-generation-{}",
                antiburn_local::analysis::pricing_generation()
            )
        }),
        old_pricing,
        replacement_pricing,
        catalog_revision: Some(antiburn_local::insights::ReportCatalogs::default().revision),
        target_model,
        target_control,
    }
}

pub(super) fn target_identity(
    secret: &[u8; 32],
    finding: &CurrentFinding,
    canonical_workspace: Option<&Path>,
) -> TargetIdentity {
    let agent = crate::agents::kind_from_slug(&finding.agent)
        .expect("the caller validates the finding agent");
    let workspace_key = canonical_workspace
        .and_then(Path::to_str)
        .map(|workspace| hashed_parts_with_secret(secret, b"workspace", &[workspace]));
    let (mut scope_kind, mut scope_key) = if let Some(workspace_key) = workspace_key.as_ref() {
        ("project".to_owned(), workspace_key.clone())
    } else {
        (
            "session".to_owned(),
            session_scope_key(secret, agent.slug(), &finding.session_id),
        )
    };
    let mut physical_target_key = None;
    let attributed = match finding.finding.cause() {
        FindingCause::OldModelUsage { .. }
            if reviewed_config_operation(agent, finding.finding.cause()).is_some_and(
                |operation| {
                    finding.effective_model.as_deref() == Some(operation.expected_value.as_str())
                },
            ) =>
        {
            Some((
                finding.effective_model_target_hash.as_ref(),
                finding.effective_model_scope.as_ref(),
            ))
        }
        FindingCause::ModelOverthinking { reasoning, .. }
            if finding.effective_reasoning.as_deref() == Some(reasoning) =>
        {
            Some((
                finding.effective_reasoning_target_hash.as_ref(),
                finding.effective_reasoning_scope.as_ref(),
            ))
        }
        _ => None,
    };
    if let Some((Some(target), Some(scope))) = attributed {
        scope_kind = scope.clone();
        scope_key = if scope == "global" {
            target.clone()
        } else {
            workspace_key.clone().unwrap_or_else(|| target.clone())
        };
        physical_target_key = Some(target.clone());
    }
    let canonical_identity = finding.finding.canonical_identity(&scope_key);
    let group_key = hashed_parts_with_secret(
        secret,
        TARGET_DOMAIN,
        &[
            &finding.environment_key,
            agent.slug(),
            &scope_kind,
            &scope_key,
            physical_target_key.as_deref().unwrap_or_default(),
            &canonical_identity,
        ],
    );
    let target_key = physical_target_key.as_ref().map_or_else(
        || group_key.clone(),
        |physical| {
            hashed_parts_with_secret(
                secret,
                TARGET_DOMAIN,
                &[
                    &finding.environment_key,
                    agent.slug(),
                    physical,
                    &canonical_identity,
                ],
            )
        },
    );
    TargetIdentity {
        group_key,
        target_key,
        canonical_identity,
        workspace_key,
        scope_kind,
        scope_key,
        physical_target_key,
    }
}

pub(super) fn session_scope_key(secret: &[u8; 32], agent: &str, session_id: &str) -> String {
    hashed_parts_with_secret(secret, b"session", &[agent, session_id])
}

pub(crate) fn passive_remediations(
    connection: &rusqlite::Connection,
    secret: &[u8; 32],
    findings: Vec<CurrentFinding>,
    boundary_ms: i64,
) -> Result<Vec<PassiveRemediation>> {
    let mut candidates = Vec::new();
    for finding in findings {
        let Some(agent) = crate::agents::kind_from_slug(&finding.agent) else {
            continue;
        };
        let canonical_workspace = finding
            .workspace_candidate()
            .and_then(|workspace| trusted_workspace_in(connection, workspace).ok().flatten());
        let identity = target_identity(secret, &finding, canonical_workspace.as_deref());
        let target = CachedTarget {
            findings: vec![finding],
            target_key: identity.target_key.clone(),
            canonical_identity: identity.canonical_identity,
            workspace_key: identity.workspace_key,
            agent,
            scope_kind: identity.scope_kind.clone(),
            scope_key: identity.scope_key.clone(),
            physical_target_key: identity.physical_target_key,
            config: None,
        };
        let definition = watch_definition(&target);
        if !watch_verification_available(
            &definition,
            &target.scope_kind,
            target.agent.slug(),
            target.findings[0].finding.detector,
        ) {
            continue;
        }
        let result = if definition.old_model.is_some() {
            json!({"version": 1, "verification": {"status": "watching", "methodRevision": VERIFICATION_METHOD_REVISION}, "savings": {"status": "pending", "methodRevision": SAVINGS_METHOD_REVISION}})
        } else {
            json!({"version": 1, "verification": {"status": "watching", "methodRevision": VERIFICATION_METHOD_REVISION}, "savings": {"status": "unavailable"}})
        };
        let snapshot = StoredDisplaySnapshot {
            version: 1,
            finding_id: stable_finding_id(&target),
            display: burn_check_display_facts(&target),
        };
        let source_generation = target.findings[0].source_generation.to_string();
        let published_fence = target.findings[0].published_fence.to_string();
        let publication_session = target.findings[0].session_id.as_str();
        candidates.push(PassiveRemediation {
            remediation_id: hashed_parts_with_secret(
                secret,
                b"antiburn/passive-attempt/v1\0",
                &[
                    &identity.target_key,
                    publication_session,
                    &source_generation,
                    &published_fence,
                ],
            ),
            target_key: identity.target_key,
            environment_key: target.findings[0].environment_key.clone(),
            agent: agent.slug().to_owned(),
            scope_kind: identity.scope_kind,
            scope_key: identity.scope_key,
            definition_json: serde_json::to_string(&definition)?,
            result_json: result.to_string(),
            display_snapshot_json: serde_json::to_string(&snapshot)?,
            boundary_ms,
        });
        if candidates.len() == MAX_PASSIVE_CANDIDATES {
            break;
        }
    }
    Ok(candidates)
}

pub(super) fn reviewed_pricing(
    agent: AgentKind,
    provider: Option<&str>,
    api: Option<&str>,
    model: &str,
) -> Option<ModelPricing> {
    let target = ModelTarget::new(
        agent.slug(),
        provider.unwrap_or_default(),
        api.unwrap_or_default(),
        model,
    );
    match ReviewedModelCatalog::default().resolve(&target) {
        Support::Supported(definition) => match definition.pricing {
            Support::Supported(pricing) => Some(pricing),
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn reviewed_replacement(agent: AgentKind, cause: &FindingCause) -> bool {
    let FindingCause::OldModelUsage {
        provider,
        api,
        model,
        replacement,
        ..
    } = cause
    else {
        return false;
    };
    let catalog = ReviewedModelCatalog::default();
    let route_target = |model: &str| {
        ModelTarget::new(
            agent.slug(),
            provider.as_deref().unwrap_or_default(),
            api.as_deref().unwrap_or_default(),
            model,
        )
    };
    let (Support::Supported(old), Support::Supported(new)) = (
        catalog.resolve(&route_target(model)),
        catalog.resolve(&route_target(replacement)),
    ) else {
        return false;
    };
    matches!(old.state, ModelState::Obsolete(rule) if rule.replacement == new.canonical_model_key)
}

pub(super) fn reviewed_config_operation(
    agent: AgentKind,
    cause: &FindingCause,
) -> Option<ConfigOperation> {
    match cause {
        FindingCause::OldModelUsage {
            provider,
            model,
            replacement,
            ..
        } if reviewed_replacement(agent, cause) => {
            let config_value = |value: &str| match agent {
                AgentKind::OpenCode | AgentKind::Pi => provider
                    .as_deref()
                    .map(|provider| format!("{provider}/{value}")),
                _ => Some(value.to_owned()),
            };
            Some(ConfigOperation {
                setting: ConfigSetting::Model,
                expected_value: config_value(model)?,
                proposed_value: config_value(replacement)?,
            })
        }
        FindingCause::ModelOverthinking {
            provider,
            api,
            model,
            reasoning,
            ..
        } if reviewed_reasoning_above_cap(
            agent,
            provider.as_deref(),
            api.as_deref(),
            model,
            reasoning,
        ) =>
        {
            Some(ConfigOperation {
                setting: ConfigSetting::Reasoning,
                expected_value: reasoning.clone(),
                proposed_value: "medium".to_owned(),
            })
        }
        _ => None,
    }
}

pub(super) fn reviewed_reasoning_above_cap(
    agent: AgentKind,
    provider: Option<&str>,
    api: Option<&str>,
    model: &str,
    reasoning: &str,
) -> bool {
    let catalog = ReviewedModelCatalog::default();
    let resolve = |effort: &str| {
        let mut target = model_control_target(agent.slug(), provider, api, model);
        target.raw_effort = Some(effort.to_owned());
        catalog.resolve(&target)
    };
    let (Support::Supported(current), Support::Supported(proposed)) =
        (resolve(reasoning), resolve("medium"))
    else {
        return false;
    };
    matches!(current.effort, Support::Supported(Some(ref value)) if value == reasoning)
        && current.family_policy.effort.above_cap.contains(reasoning)
        && matches!(proposed.effort, Support::Supported(Some(ref value)) if value == "medium")
        && !proposed.family_policy.effort.above_cap.contains("medium")
}

pub(super) fn config_setting_from_name(value: &str) -> Option<ConfigSetting> {
    match value {
        "model" => Some(ConfigSetting::Model),
        "reasoning" => Some(ConfigSetting::Reasoning),
        _ => None,
    }
}

pub(super) fn evidence_guard(finding: &CurrentFinding) -> RemediationEvidenceGuard {
    RemediationEvidenceGuard {
        environment_key: finding.environment_key.clone(),
        agent: finding.agent.clone(),
        session_id: finding.session_id.clone(),
        source_generation: finding.source_generation,
        published_fence: finding.published_fence,
        source_fingerprint: finding.source_fingerprint.clone(),
        processed_fingerprint: finding.processed_fingerprint.clone(),
        parser_revision: finding.parser_revision,
        analyzer_revision: finding.analyzer_revision,
        evidence_schema_revision: finding.evidence_schema_revision,
    }
}

pub(super) fn public_watch(
    store: &Store,
    record: &RemediationRecord,
) -> Result<Option<WatchStatus>, ControllerError> {
    let Some(snapshot) = store
        .remediation_display_snapshot(&record.remediation_id)
        .map_err(|_| ControllerError::PersistenceFailed)?
    else {
        // A missing snapshot identifies a legacy watch without inventing display or origin facts.
        return Ok(None);
    };
    let result = stored_result(&record.result_json).map_err(|_| ControllerError::Internal)?;
    parse_display_snapshot(&snapshot.display_snapshot_json)
        .map_err(|_| ControllerError::Internal)?;
    let origin = snapshot.origin;
    Ok(Some(WatchStatus {
        watch_id: record.remediation_id.clone(),
        origin: match origin.as_str() {
            "passive" => RemediationOrigin::Passive,
            "action" => RemediationOrigin::Action,
            _ => return Err(ControllerError::Internal),
        },
        lifecycle: record.state,
        verification: result.verification,
        savings: result.savings,
    }))
}
